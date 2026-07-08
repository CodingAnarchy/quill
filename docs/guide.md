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

The usual editing keys work in the default (non-Vim) mode: Backspace deletes the
character behind the cursor and Delete the one ahead; Ctrl-Backspace (or
Alt-Backspace) deletes the whole word behind it. The arrows move by character
and line, Home/End jump to the ends of the row, Ctrl-← and Ctrl-→ move a word at
a time, and PageUp/PageDown move a screenful. The mouse works too: click to
place the cursor, drag to select a passage, and scroll the wheel to scroll the
page. Copying works with the system clipboard: a selection you copy (or a Vim
yank/delete) is placed on the OS clipboard — over SSH and tmux as well — and your
terminal's paste (Ctrl-Shift-V / Cmd-V / middle-click) inserts the clipboard at
the cursor as one undoable edit. (Vim mode, if enabled, keeps its own bindings.)

Markdown is understood as you write: emphasis (*italic*, **bold**) renders
softly, with the markers hidden until you edit the span, and Ctrl-O toggles an
in-chapter outline pane. A scene break renders as a centered typeset divider
rather than its raw marks, revealed back to the marks only when the cursor is on
that line; the paragraph after a break (or a code block) is set flush-left, the
way the exports typeset a new scene.

Fence a block in ``` to set it apart verbatim — a log readout, a transmission,
a letter, anything whose line breaks and spacing should be preserved exactly. In
the editor the block shows as a set-apart, tinted box: the ``` fence lines are
hidden and the content is shown exactly as typed, whitespace and line breaks
preserved rather than reflowed like prose — a true preview of how it will appear.
Move the cursor into the block and the raw ``` fences reappear so you can edit
the markup, just as emphasis markers reveal on the span you're editing; move out
and it returns to the preview. The block carries through to every export. (Smart
typography still applies inside: these blocks hold prose, not source code.)


CHAPTERS & NAVIGATION

A project is a folder of chapters (one Markdown file each) plus a book.toml
manifest. Ctrl-N and Ctrl-P step to the next and previous chapter. Ctrl-G opens
the chapter browser: arrow keys move, Enter opens the selected chapter, "o"
opens it in a side pane, and "f" lists the project's reference files (the story
bible and loose notes, grouped so a large set is easy to scan). A reference pane
is a normal editor — focus it with Ctrl-W w or a mouse click, drag its divider to
resize, and close it with Ctrl-W c or Vim ":q". Press "/" to filter the list as you type (by title,
status, or POV) and quick-jump to a chapter; Enter opens the highlighted match,
Esc keeps the filter for the browser's commands to act on, and a second Esc
clears it. Press "#" to go straight to a chapter by number (1-based) or title
without scrolling the list. From the browser you can also create, rename, reorder, and delete
chapters, split the open chapter at the cursor into two ("x"), merge a chapter
into the one above it ("M"), duplicate a chapter ("c"), and set each chapter's
status, summary, and section.

Ctrl-L toggles the continuous-manuscript view, which flows across chapter
boundaries and writes edits back to the right files, while never holding more
than a few chapters in memory. Faint part and front/back-matter section headers
mark each structural transition as you flow through it (render-only, like the
chapter dividers), so you can tell which part you're in.

Ctrl-W begins a window command for split panes: split the editor, open another
chapter or a bible entry or a docked report beside the prose, switch focus,
resize, and close. (See docs/windows.md for the full set.)

Run "Corkboard" from the Ctrl-K menu for an index-card overview of the active
chapter's scenes — one card per ## / ### heading, in order. Arrow keys move
between cards; Enter jumps the editor cursor to that scene. It's also a planning
surface: press K or J to move a scene up or down (its heading and prose move
together, and the chapter is rewritten and saved on the spot), "e" to write a
one-line synopsis for the selected card, and "s" (or Space) to cycle its status
— stub, draft, done. Each card shows a status badge and its synopsis (falling
back to the scene's first line of prose until you write one). The synopsis and
status are kept in the chapter's front matter, out of the prose, and travel with
their scene when you reorder — so they survive reopening the project and never
touch the manuscript text. (The corkboard is a single-chapter feature; leave the
continuous view with Ctrl-L first.)


PROJECTS

Ctrl-B opens the project browser so you can move between books without quitting.
It lists your recently-opened projects, newest first; Enter switches to one in
place (saving the current project first, and confirming if there are unsaved
changes). Press "b" to browse the filesystem for a project that isn't listed
yet, or "n" to create a brand-new one — type its path and Quill makes the
folder, manifest, and first chapter, then opens it.

At library scale the list can grow long, so "/" filters it the way the chapter
browser does: type to narrow to the projects whose title or folder path matches,
the arrows move within the matches, and Enter opens the highlighted one. Esc
leaves the filter in place (so the arrows still act on the narrowed list); a
second Esc clears it. You can also prune the library from here: "r" renames the
selected project's folder (Quill moves it and updates your recents), and "d"
deletes it after a confirmation — that removes the folder and all its files, so
it asks first. The project you currently have open is never deleted from the
browser (switch to another first), and renaming the open project routes through
the in-place rename so the editor follows the folder.

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

If you always return to the same book, set open_last = true under [projects] and a
bare `quill` reopens your most-recently-opened project directly instead of showing
the chooser. It only affects a no-path launch from outside a project; an explicit
path and in-project launches are unchanged, and with no recent project to reopen it
falls back to the usual chooser (or new-book prompt).

Run "Book details" from the Ctrl-K menu to set the book's title, subtitle,
author, and series — the fields that flow into every export's title block —
followed by its publishing metadata: ISBN, publisher, publication date, language,
and the rights/copyright line. Quill walks them in turn, each prompt prefilled
with its current value: press Enter to keep a field and move to the next, type to
change it, or Esc to stop. The title keeps its current value if you leave it
blank; every other field clears when you leave it blank. (The title, subtitle,
author, and series are the same fields the chapter browser sets one at a time
with "t", "b", "w", and "e"; either way writes the same manifest.)

The publishing metadata rides into the exports a published book needs. In the
EPUB OPF it becomes the `dc:identifier` (your ISBN when set, otherwise a stable
minted id), `dc:publisher`, `dc:date`, and `dc:rights`, and the language drives
`dc:language` and the `xml:lang` markup throughout (defaulting to `en` when you
leave it unset — set it for a non-English book). When any of the rights line,
publisher, publication date, or ISBN is set, Quill renders a copyright page as
front matter — after the cover and title page, before the body — in the EPUB,
PDF, and Word (manuscript) exports. All of these round-trip through `book.toml`,
so they survive a reopen and an export. The publication date accepts a year
(`2026`) or a full date (`2026-06-30`); the language is a BCP-47 tag (`en`,
`fr`, `es-419`, …).

To rename a project, run "Rename project" from the Ctrl-K menu: it's prefilled
with the book title, and renaming moves the folder and updates the book title
together so they stay in sync. The move happens without quitting, and the project
keeps saving to the new location. (You can still set the title on its own with
"Book details" in the Ctrl-K menu, or "t" in the chapter browser.)

A project can hold a series of books. When it does, Quill opens the first book;
run "Switch book" from the run-action menu (Ctrl-K) to move to another — the
editor, chapter list, and reports follow the book you pick, and the chapter
you're leaving is saved first. A standalone novel is just a one-book project, so
the command says there's only one book.

To build a series in the first place, run "Manage books (series)…" from the
Ctrl-K menu (the "book:manage" action). On a standalone novel it offers to start
a series: name the series (blank uses the current book's title), and the book you
have becomes book one — then it prompts for the second book's title straight
away, since a series only earns its keep with more than one book. Each book you
add starts with an empty chapter and becomes the active book. On a project that's
already a series, "Manage books" lists every book with entries to switch to it,
move it up or down in reading order, remove it, or add another. Removing a book
is confirmed first and never deletes your work — its chapter files move to the
project's `unused/` folder, the same as removing a chapter — and the active book
stays consistent (removing the one you're in drops you into a neighbour).
Reordering only changes the book order in the series; no files move. Every change
is written to the `[[books]]` manifest in `book.toml`, so hand-editing it still
works and round-trips.

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
Esc — turn it off with "setup_wizard = false" under [projects]. On a first run a
dismissible quick-start card also points at the core keys and next steps, so a
new project isn't a blank page with no signposts.


SAVING & SAFETY

Ctrl-S saves the active chapter (or the continuous window). Saves are atomic: an
interrupted save never corrupts a file, and the previous version always
survives. If Quill exits unexpectedly, the next launch offers back any unsaved
work. Ctrl-Q (or Ctrl-C) quits, prompting first if there are unsaved changes.

On a clean save or quit, Quill also remembers where you left off — the active
chapter and the cursor — and reopens the project there next time, rather than at
chapter one, top of file. This is separate from crash recovery above: recovery
restores unsaved *text* after an unexpected exit; the resume point restores your
last *location* after a clean one. Neither changes the other.

The status bar shows a save-state marker beside the chapter name: ✓ when the
buffer is on disk, ● when you have unsaved changes. Set [editor]
autosave_interval_secs = N in your config to save automatically after N seconds
of idle (0, the default, is off); autosave uses the same atomic path as Ctrl-S
but doesn't run on_save workflows.

For a coarser checkpoint than the per-chapter save/recovery, "Milestones" (in the
Ctrl-K menu) marks a named point in the whole project's history — "first draft
done", say — that you can come back to: restore the entire project to a milestone,
or diff the current manuscript against one to see everything that's changed since.

Quill guards your files against the ways they can go wrong quietly: an atomic save
through a symlinked chapter replaces the real target rather than orphaning it; a
project you open while another Quill already has it opens read-only (an advisory
lock keeps two processes from racing writes and clobbering the shared sidecars),
while a lock left behind by a crashed or force-killed session is detected as
stale and reclaimed automatically, so you're never locked out of your own
project; and a transient read error is told apart from an empty file, so a hiccup
never zero-word-caches a chapter or wrongly offers recovery.


ARCHIVE & BACKUP

To keep an offline backup — no git, no remote, no account — run "Archive
project…" from the Ctrl-K menu (or "archive"). It writes a single .zip of the
whole project: the manifest, every chapter, the story bible, your annotations,
and the custom dictionary. The prompt is prefilled with a timestamped file name
(your project's folder name plus the date and time, e.g. My-Novel-2026-06-30-
142530.zip) in your projects folder, so a fresh backup is usually one Enter away;
edit the path to save it anywhere you like — a USB stick, a synced folder,
wherever. The active chapter is saved first, so the archive always reflects
what's on screen. Editor scratch is left out (the .quill/ recovery state, the
unused/ folder of removed chapters, the export/ folder, and OS cruft), so a
backup carries the manuscript, not the leftovers.

To bring one back, run "Restore project from archive…" (or "archive:restore"):
point it at a .zip, then give it a new folder to unpack into. Quill reproduces
the project there and opens it. It won't overwrite an existing project — pick an
empty folder — so a stray path can never clobber work in progress.

If the project is under version control and points at a remote (Ctrl-V →
"Set remote"), Quill can keep that remote as an always-current off-site backup so
you never have to remember to push. It's opt-in per project: add a [backup]
section with enabled = true. By default it then pushes after each save; set
interval_mins = N to also push at least every N minutes during a long session
(and on_save = false to rely on the timer alone). The push runs in the background
and never blocks editing; when it can't reach the remote — offline, a login
problem, or the remote is ahead — the status bar says so and Quill retries on the
next cadence rather than losing the attempt. Once a backup succeeds, the status
bar shows how long ago ("git · backed up 2m ago"). With backup off (the default)
or no remote set, nothing changes — the remote is only ever updated by an explicit
Push.

The same [backup] section can also keep scheduled *local* archives — the same
one-Enter "Archive project…" zip, but on a cadence and with no git or network
needed. Add local_enabled = true and local_interval_hours = N, and Quill zips the
whole project into ~/.local/share/quill/backups/<project>/ at least every N hours
while it's open, keeping the local_keep most recent (5 by default; 0 keeps every
archive) and pruning the rest. It runs in the background off the editing thread; a
failure is surfaced, not silent, and retried on the next cadence. Once one lands,
the status bar shows how long ago ("saved 20m ago"). Open a project whose newest
local archive is older than stale_warn_hours (24 by default; 0 turns the warning
off) and Quill warns you up front that the copy is stale. Local backups are off by
default and independent of the remote push — you can have either, both, or neither.

You can browse and restore those local archives without leaving Quill. "Browse
backups…" (Ctrl-K, or backup:browse) lists the archives kept for the project so
you can inspect and restore one in place, and opening a project whose newest local
backup is more recent than the working copy prompts to restore it — recovery is a
confirmation, not a manual unzip.


REPORTS

Ctrl-R opens the progress report: completion against your goals, a projected
final length and finish date from your writing pace, and today's word delta. Set
a deadline date under [goals] and the projection is deadline-aware — it shows the
daily pace your target date implies against the pace you're keeping.
Set your goals without leaving Quill: run "Set writing goals" from the Ctrl-K
menu (or "goals:set"). It walks you through the target word count, the target
chapter count, the daily word target, and the words-per-page used for the page
counts — each prefilled with its current value. Type a positive number to set
one; leave the daily target blank to clear it (the report's daily line then
disappears). The changes are written to the [goals] table of book.toml and the
report reflects them immediately. The report itself shows the same hint at the
bottom, so you can change a goal right after reading where you stand. Ctrl-T
opens the analysis report: per-chapter readability, pacing, sentence lengths, and
overused words and phrases. Both are scrollable; press s to save the report you're
reading to a dated Markdown snapshot (export/progress-<date>.md or
export/analysis-<date>.md) for your records or to share; any other key closes them.

Run "Writing history" from the Ctrl-K menu (or "report:history") to see the
recorded per-day history Quill keeps as you write: your current and longest
writing streak (a day counts when its net words are positive; a streak survives a
single day off but lapses after that), your best day, the totals across every
recorded day, and a sparkline graph of daily net words — green bars for gaining
days, red for losing ones — under the date range it spans. It only reads the
history log; nothing new is recorded. Like the other reports, it's scrollable and
any other key closes it. The same view carries a month calendar heatmap of your
daily activity and your per-session records — the words, elapsed time, and
words-per-minute of each writing session Quill logs.


EXPORT

Ctrl-E opens the export menu: Markdown (one compiled document), Word (.docx in
standard manuscript format), Scrivener (a .scriv bundle), Vellum (a .docx tuned
for Vellum import), EPUB, and PDF (screen and print-interior). Files land under
export/. For a multi-book series, each format is offered twice — the whole series
(books in reading order, delimited per format) or just the active book.

To hand the manuscript to a beta reader or an editor for markup, run "Export
review copy" from the Ctrl-K menu (the "export:review" run-action). Unlike the
"final" exports, the review copy is built to be marked up: it's a .docx with every
line numbered down the margin and generous 2-inch margins for notes, saved to
export/<book>-review.review.docx. It's also the one export that carries your
inline annotations (Ctrl-K "Annotate selection") — every other export strips them,
but the review copy turns each note into a Word comment anchored to the exact
phrase you left it on, so your reader sees your questions in context. (Like the
other exports, run it from the single-chapter view, not the continuous view.)

When the review copy comes back marked up, "Import review copy" (also in the
Ctrl-K menu) reads the returned .docx and turns your editor's Word comments into
inline annotations, anchored to the exact phrase each one sits on — the round
trip from export to editorial feedback, with the notes landing where you can act
on them. "Review annotations" (Ctrl-K) opens the notes as a list you can work
through: reply to a note and resolve it when it's handled, and — when a
manuscript comes back marked up by several reviewers — sort the list, filter by
reviewer or status, and jump to the next open one.

The export menu also leads with "Set export style…" and "Preview style…". The
designed exports (EPUB and print PDF) can be lightly styled, and you no longer
have to hand-edit book.toml to do it: "Set export style…" (also the "style:set"
run-action) opens a menu of the common knobs, each showing its current value —
pick a built-in look from the preset library (classic, modern, literary, romance,
thriller, fantasy, minimalist), set the scene-break glyph, heading alignment, drop
caps, and the title/part ornaments, and choose the print-interior trim size (6x9,
5x8, or 5.5x8.5). Each choice is written straight into the [style] / [print]
tables of book.toml, leaving the rest of the file (other sections, comments,
formatting) untouched, and takes effect on the next export and in the preview with
no restart. "Preview style…" renders a fixed sample book through your current
[style] and opens it in your system EPUB reader, so you can see a look before
committing. Hand-editing book.toml still works and round-trips; for fonts and
image ornaments, set a file path by hand or use "Browse & download fonts…" for the
curated open-licensed library. (Setting the style from the app is for a standalone
novel; a multi-book series sets [style] / [print] per book by hand.)

Quill also generates two standard front-matter pages on request, the same way it
already generates a copyright page from your publishing metadata. Set half_title =
true in the [style] table to add a bare half-title page — the book title alone —
ahead of the full title page, the conventional first printed leaf of a trade book;
it's off by default. And when you export one book of a multi-book series (its
[[books]] list), Quill adds an "also in this series" page listing the other books
by title; a standalone novel has no other titles, so it never gets one. That page
is on by default for a series; set series_list = false in [style] to suppress it.

Before you export for publication, run "Export preflight (publish health check)"
from the Ctrl-K menu. It checks the book against what print/ebook stores (KDP,
IngramSpark) reject or flag and gathers the result into one auditable checklist —
each line marked PASS, WARN, or FAIL: a missing ISBN, no cover image or a cover
under the 300-DPI print floor, no copyright page, a language tag that isn't valid
BCP-47 (use "en", "fr", "es-419", not "English"), print bleed set with no
full-bleed cover to fill it, empty chapters, and stray (unbalanced) emphasis
markers that would print a literal asterisk. The report opens on screen and is
also saved to export/preflight-<date>.md so you have a dated record to fix against.
It's a report only — it validates and never changes your export.

When you're ready to publish, "Prepare for KDP" and "Prepare for IngramSpark"
(from the Ctrl-K menu, or the "export:kdp" / "export:ingramspark" run-actions) do
the whole print-on-demand bundle in one step. Each sets the sane defaults those
stores want — a 6x9 trim if you haven't picked one, a 0.125" full-bleed
allowance, and a default "en" language if you never set one (your ISBN,
publisher, and rights are left as you have them) — writes them to book.toml, then
runs the preflight above and saves it to export/preflight-<date>.md. If the
preflight comes back clean (or with warnings only), it emits both publishing
artifacts together: the print-interior PDF (export/<book>.print.pdf) and the
reflowable EPUB (export/<book>.epub). If the preflight *fails* — something a store
rejects outright, like a missing ISBN — nothing is exported and Quill points you
at the saved report to fix first, so you never hand a store a bundle it would
bounce. (Like setting the style from the app, the one-click prepare is for a
standalone novel; a multi-book series sets [print] and exports per book.)

The print-interior PDF is only half of a print-on-demand book — the block that
goes between the covers. The other half is the wrap cover, the single sheet that
folds around the whole book: back cover, spine, and front cover in one PDF.
"PDF wrap cover (front + spine + back)" in the export menu (or the
"export:pdf-wrap-cover" run-action) generates it. The sheet is sized to
2×trim + spine + bleed wide and trim + bleed tall — big enough that, once the
printer trims and folds it, the front and back land exactly on your trim size and
any full-bleed art runs clean off the edge. Your cover image (the same one the
interior and EPUB use) fills the front panel; the back panel carries a reserved
barcode/ISBN box in its lower-outside corner for the retailer's barcode, and thin
guides mark where the cover folds around the spine. It's written to
export/<book>.cover.pdf as a PDF/X-3 grayscale press file, the same conformance as
the interior.

The spine width is the part you can't eyeball: it's the interior page count times
the paper's per-page thickness (its caliper). Quill lays your interior out at your
trim and bleed to count the pages, then multiplies by the caliper, so the spine
always matches the exact block that ships beside it — add a chapter and re-export
and the spine widens to suit. The default caliper (0.0025"/page) suits typical
uncoated book stock; if your printer publishes a different figure for your paper,
set caliper = 0.00225 (or whatever they list, in inches) in the [print] table and
the spine follows. Like the one-click prepare, the wrap cover is a standalone-novel
artifact — a series wraps each book on its own page count, so export it per book.


AI ASSISTANCE (opt-in)

AI is off by default. The first time you press Ctrl-A (or run any AI action from
the Ctrl-K menu) Quill offers to enable it right there: confirm the y/N prompt and
it writes the setting for you, then asks for your API key. You can also run
"Enable AI assistance" from Ctrl-K. Decline and AI stays off — nothing changes.

With AI enabled, Ctrl-A opens the AI command palette. The
assistant is a helper, never a co-writer: it tracks, outlines, marks, and
observes, but it never writes or rewrites your manuscript prose. It can build a
story bible (characters, timeline, setting, plot threads, and a theme/motif
tracker that follows a book's recurring themes), flag continuity and naming
issues, observe style, brainstorm ideas, draft a starter cast or setting from a
concept, and produce off-manuscript material (a synopsis, query letter, blurbs,
social posts). Revision triage collects every finding into one list to work
through, and the "Manuscript health" dashboard gives a one-screen overview of
the open findings grouped by kind (continuity, consistency, style, and the
rest); you can export the findings list to a file and re-check every open
finding in one pass. A finding corroborated across passes is marked as
consensus, and dismissing one persists against its anchored span, so it stays
dismissed when you re-run.

"Ask the manuscript" retrieves the most relevant chapters for your question
through an on-device semantic index, so its grounded, citable answer points at
the right passages without a cloud round-trip. The same index powers more than
Ask: "find similar scenes" locates the scenes closest in meaning to a passage
or query, echo detection flags passages that read too much like each other (as
findings you can revise), a motif & character co-occurrence report shows which
motifs and characters recur together, and the incremental continuity and setting
passes pull their context from the index rather than a fixed window. An
author-written research notebook kept in the project can be cited by the bibles,
so a tracked fact traces back to a source you control.

Each pass runs off the editor thread, so the editor never freezes, and a pass
that would cost more than your threshold asks before spending. Passes route to
the right-sized cloud model per action, you can run several as a durable queue
that resumes where it left off after a restart, and — opt-in — a background
bible update rebuilds the bibles from the full manuscript through Claude's
asynchronous Batch API (about 50% cheaper), returning immediately and
reconciling the result on a later launch (set [ai] background-batch-update; off
by default).

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
the cursor verbatim, as a single undo. Put $0 anywhere in the text to set where
the cursor lands (the marker itself is not inserted) — e.g. "“$0”" drops the
cursor between the quotes. Only the first $0 counts; write $$0 for a literal $0.


KEY REFERENCE

  F1              this guide
  Mouse           click to position the cursor; drag to select; scroll-wheel to scroll
  Paste           terminal paste inserts the system clipboard (one undoable edit)
  Ctrl-S          save
  Ctrl-Q / Ctrl-C quit (prompts if unsaved)
  Ctrl-Z / Ctrl-Y undo / redo
  Ctrl-F          find (then "r" to replace)
  Ctrl-O          toggle the chapter / outline pane (press again to focus it,
                  then Enter jumps to the selected heading)
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
turn it off in Settings (below) or set [editor] smart_typography = false.

Vim-style modal editing is available, off by default; enable it in Settings or
with [editor] vim = true.

In Vim mode, marks and a jump-back list keep your place across the big jumps. In
Normal mode, "m" then a letter sets a mark at the cursor (mx); a backtick or a
single-quote then that letter jumps to it (`x or 'x). Marks are global — a mark
carries its chapter, so it returns you to the right chapter even after you've
moved to another. The jumps that used to strand you — the outline, the corkboard,
find and search (/, ?, n, N, *), and go-to-chapter — now record where you left,
and a double backtick (``) or double quote ('') jumps back to it, popping one
step each press. (Vim's usual Ctrl-O back-jump is taken by the outline pane, so
Quill uses `` / '' instead.)


SETTINGS

Run "Settings…" from the Ctrl-K menu (or the "settings" action) to change the
everyday preferences without hand-editing config or turning on AI. The menu
shows each setting's current value; pick a row to change it:

  Theme               dark or light (applies immediately)
  Column width        the centered text column, 32–160 characters
  Smart typography    curly quotes / em-dashes / ellipses as you type
  Vim mode            modal editing
  Autosave interval   seconds of idle before an automatic save (0 is off)
  Typewriter scrolling pin the cursor's line to the page centre
  Focus mode          dim every paragraph but the one under the cursor
  Paragraph indent    indent the first line of each prose paragraph (off by
                      default; display-only, and matches how the exports look —
                      headings, dividers, epigraphs, code, and the first
                      paragraph after a heading are never indented)
  Analysis stopwords  extra words to exclude from the analysis report's
                      most-used-words list (space-separated; blank clears them)

Changes apply live — no restart. The editor/theme settings are written to your
~/.config/quill/config.toml (preserving the rest of the file); the analysis
stopwords are written to the project's book.toml. If you'd rather describe the
change in words and let the AI map it (AI required), "Configure by description"
does the same thing conversationally.

PER-PROJECT SETTINGS

Your ~/.config/quill/config.toml holds the global settings shared across every
project. Every setting it can hold — each key's default, valid values, and an
example — is documented in the configuration reference (docs/configuration.md).
A single book can override any of them for itself in its book.toml
manifest, under a [quill] table — a screenplay can pin a wider column, a light
theme, or its own workflow set without disturbing your defaults. The override
sections mirror the global file's, nested under [quill]:

  [quill.editor]
  column_width = 90
  vim = true

  [quill.theme]
  name = "light"

The same works for [quill.snippets], [quill.workflows], and [quill.projects].
Precedence is per field: a key set under [quill] wins; a key it leaves unset
falls back to the global value; and a key neither file sets falls back to Quill's
built-in default. So a project override lists only the settings it wants to
change — everything else still tracks your global config. A book.toml with no
[quill] table behaves exactly as before, using the global settings throughout.

The override lives in book.toml (alongside [goals] and [analysis]) rather than
the project's git-ignored .quill/ state directory, so a project's settings are
committed and travel with the book. The global file stays the base — your AI keys
and other private settings live there, never in a committed book.toml. Project
overrides are hand-edited; the in-app Settings menu always writes the global file.
