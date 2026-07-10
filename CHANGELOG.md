# Changelog

All notable changes to Quill are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.0] - 2026-07-10

The Revision Desk. Quill has always been a strong tool for *drafting*; this
release makes the second and third pass — proofing, tightening, restructuring,
and hitting goals — just as first-class and keyboard-fast. Repeat an edit with
`.`, join and reflow, transpose and case-fix, auto-pair quotes; step through the
style report's flags in the buffer like a spelling sweep and catch words echoed a
few sentences apart; jump by line, scene, or annotation and trace your recent
edits; restructure a chapter from the outline; read a galley in a paginated
proofing mode; watch live word goals fill and get a quiet session recap. It's all
author-driven mechanics: Quill flags, jumps, counts, moves, and formats — the
words stay yours. Alongside the editor work, the AI gains an opt-in **cultural &
historical accuracy** pass (author-invoked, findings only — accuracy you ask for,
never a sensitivity or representation review), and the version-control workflow
shipped in 0.7.0 is now fully documented. Before release, the new
gestures — folding navigation, cross-chapter moves, the reflow/restructure
paths — went through an adversarial reliability audit; the fixes below close a
reachable crash and two continuous-view data-loss edge cases it surfaced.

### Added

- **Repeat the last change with `.`** — Vim's repeat command, for both Normal-mode
  changes and Visual-mode operations, so a repetitive revision edit (cut a filler
  word, fix a punctuation habit) is one keystroke instead of a manual re-do.
- **Step through the style report's flags in the editor** — walk each occurrence
  of a chosen category (passive voice, filter words, long sentences, over-used
  words) with `]r`/`[r`, the current hits underlined in place like a misspelling —
  navigation only, never a rewrite.
- **Flag near-proximity word repeats** — surface a content word echoed within a few
  sentences (the local repetition a global word count misses) as a dim mark while
  you revise; toggleable, purely lexical and offline.
- **Join lines and reflow a paragraph** — `J` joins, `gq`/`gw` re-wraps the
  paragraph, collapsing ragged line breaks after a cut. Whitespace only; never
  alters a word or crosses a scene/chapter boundary.
- **Transpose and case-fix** — `gs`/`gS` swap characters/words (the classic `xp`),
  `~`/`g~`/`gu`/`gU` toggle or set case over a motion or text object, for fast typo
  cleanup.
- **Opt-in auto-pair for quotes and brackets** — typing an opener inserts its closer
  with the cursor between; over-typing a closer skips past it. Off by default
  (`[editor] auto_pair`), composes with smart typography.
- **Restructure headings and sections from the outline** — promote/demote a heading
  (`<`/`>`) and move a section with its whole body (`K`/`J`) without opening the
  corkboard; one undoable, gesture-scoped rewrite.
- **Scene-heading and open-annotation motions** — `]]`/`[[` step to the next/previous
  scene heading and `]a`/`[a` to the next/previous unresolved annotation, across
  chapters, without opening an overlay.
- **Go to a line, and step back through recent edits** — a go-to-line prompt plus a
  bounded ring of recent edit locations (and a `gi`-style jump to the last
  insertion) so returning to where you were is automatic.
- **Live per-chapter and per-session word goals** — a chapter fills toward its stored
  goal as you write, and an optional session target fills alongside it, updated
  from cached counts with no per-frame file work.
- **Session recap and a gentle below-target nudge** — a quiet summary of the session
  and the day's net on quit, and an optional dismissible status-bar cue when the
  day is under the `[goals] daily` target.
- **Word-level diff in the history and milestone compare view** — a changed line now
  highlights the specific words that differ, so a prose revision reads the way a
  novelist thinks about it rather than lighting the whole line.
- **Preview per-chapter match counts before a project-wide replace** — a manuscript-wide
  find-and-replace shows how many matches are in each chapter, with context, to
  confirm or cancel before it rewrites files.
- **Paginated galley proofreading mode** — a distinct read-only mode that renders the
  chapter one screen-page at a time (with optional line numbers), so a proofing
  pass reads like a printed galley instead of a scroll.
- **Opt-in cultural & historical accuracy analysis** — an author-invoked pass that
  checks the manuscript against a concern you *name* ("1850s London", "18th-century
  sailing") and flags passages that read as factually, period, or culturally
  inaccurate for it — as findings you act on, never a rewrite. It checks only the
  concern you asked for; it is deliberately **not** a sensitivity, representation,
  or bias review.

### Changed

- **Version-control workflow is now documented** — the README and a new
  `docs/version-control.md` cover two-way sync, drafts, in-app conflict resolution,
  the branch/merge history graph, and milestones, so the 0.7.0 suite is
  discoverable instead of implied to be push-only.
- **Internal quality pass** — the oversized `vcs` modules and five other files past
  the size guideline were split into cohesive submodules, and the `cargo perf` gate
  was made hermetic (the network model-catalog check no longer runs with the timing
  guards) and contention-resilient, so a green perf run is reproducible offline.

### Fixed

- **`J`/`gq` no longer crash, corrupt, or mangle across boundaries** — a reachable
  panic on a paragraph ending in a multibyte character with no trailing newline, a
  continuous-view join/reflow that duplicated the next chapter's text across the
  seam, and reflow folding a heading into adjacent prose are all fixed.
- **Outline restructure and jump-back respect the continuous view** — restructuring
  is refused in the continuous view (it previously edited and saved an invisible
  stale buffer, losing on-screen edits), and the recent-edit ring / marks now record
  the correct chapter and offset there.
- **Navigating into a folded section keeps the cursor visible** — stepping a style or
  spell flag, `:{line}`, and the annotation-review jump now reveal the fold at the
  cursor, and switching chapters clears the previous chapter's highlight caches so no
  phantom marks render.
- **Vim edit-mechanics edge cases** — `.` no longer misfires while a `g~`/`gu`/`gU`
  operator is armed, a no-op case/transpose no longer marks the buffer dirty or
  retargets `.`, and a failed operator+motion (`df<absent>`) is now a true no-op.
- **Revision highlights no longer lag while typing** — the style-flag and
  proximity-repeat caches are invalidated on edit, so a mark never briefly sits on
  shifted offsets during an active typing burst.
- **Word-diff is guarded against pathological line lengths** — a very long single-line
  paragraph in the compare view falls back to plain line coloring instead of building
  a huge LCS table, and each line pair is diffed once rather than twice.

## [0.7.0] - 2026-07-09

Version control for prose. Quill now backs the manuscript with real git-grade
history that reads like a writing tool, not a developer one: write on two
machines and reconcile, keep parallel drafts of a chapter and merge the one you
like, and resolve any overlap chapter-by-chapter without leaving the editor.
Alongside it, structural authoring gets sharper — move scenes across chapters on
the corkboard, fold a long chapter down to its scene headings (or drop into a
distraction-free composition mode), jump anywhere by fuzzy name, and read
project-wide find results grouped by chapter. Before shipping, the new
sync/merge, cross-chapter move, and folding paths went through an adversarial
reliability audit; the fixes below close every data-loss and merge-failure edge
case it surfaced, so the two-machine and multi-draft workflows are safe on real
manuscripts (nested story-bible files and all).

### Added

- **Two-way sync** — fetch and merge remote chapter edits, not just push, so two
  machines (or two people) can each write and reconcile their changes into one
  history.
- **In-app conflict resolution** — when a sync or draft merge overlaps, resolve
  it chapter-by-chapter inside Quill (keep mine, keep theirs, or open the
  conflicting versions) instead of dropping to a git command line.
- **Branch-as-draft** — keep parallel drafts of the manuscript as named branches
  you can create, switch between, list, and merge, so you can try an alternate
  version of a chapter and fold back the one you keep.
- **Branch/merge commit graph in history** — the history view draws the branch
  and merge structure of the project's timeline, so a manuscript with drafts and
  merges is legible at a glance.
- **Milestone notes** — attach an optional note to a milestone to record what it
  marks, so a tagged point in the history carries its own context.
- **Actionable remote-error taxonomy** — sync/push failures are triaged into
  clear categories (auth, rejected, offline, other) with guidance, instead of a
  raw libgit2 message.
- **Move scenes across chapters on the corkboard** — drag a scene card from one
  chapter to another and both chapter files are rewritten together atomically,
  with undo, so restructuring across chapter seams is a single safe gesture.
- **Heading/scene folding and distraction-free composition** — collapse a chapter
  to its headings (or a section under a heading) to navigate a long chapter by
  structure, and toggle a distraction-free composition mode for drafting.
- **Fuzzy chapter/scene quick-switcher (Ctrl-D)** — jump to any chapter or scene
  heading across the project by typing part of its name.
- **Grouped results pane in project find** — project-wide find results are grouped
  by chapter, so a match list across a large manuscript is easy to scan and jump
  through.

### Changed

- **Internal quality pass** — five oversized modules were split into cohesive
  submodules (`mode`, `project/series`, the spell-check methods out of `app`,
  `ui/list_overlay`, and `ai/embed`), keeping files under the size guideline as
  the version-control surface grew.

### Fixed

- **Sync and draft-merge on real projects with a story bible** — the three-way
  merge built the merged tree with a flat builder that rejected nested paths, so
  any diverged sync or draft merge touching a `bible/…` (or other nested) file
  failed. It now builds through the git index and handles nested files, with the
  merge exercised across a story bible.
- **Cross-chapter scene move no longer risks prose on a read error** — a move now
  aborts (and records no undo) if a chapter can't be read, instead of composing
  the destination onto empty content and overwriting real prose; the multi-file
  swap also rolls back on a partial failure and preserves a symlinked chapter.
- **Sync conflicts never clobber the current view or misroute a resolution** — a
  background sync that lands with conflicts is deferred until you're back in an
  editing context rather than overwriting an open prompt or overlay, and a stale
  pending draft merge can no longer route a sync resolution to the wrong refs.
- **No silent drop when the remote moves mid-resolution** — if the remote gains a
  new conflicting edit while you're resolving a sync, Quill now aborts and asks
  you to sync again instead of quietly discarding the remote change.
- **Cursor stays visible when navigating into a folded section** — jumping or
  arrowing into a collapsed region reveals it so the caret is never stranded on a
  hidden line, and clicking a collapsed heading now lands on the right character.
- **Background git operations respect the read-only instance lock** — sync, pull,
  and draft checkout refuse to run from a secondary read-only instance, mirroring
  the save-path guard.

## [0.6.0] - 2026-07-08

A semantic-manuscript release, built on an on-device embedding index of the
book. Quill can now find scenes by meaning, catch passages that echo each other,
and feed the right nearby prose into its continuity and setting passes instead of
guessing. The AI-assist surface hardens inside the assistant boundary: findings
gain cross-pass consensus and durable, span-anchored dismissals; the story bible
can cite an author-written research notebook; multi-pass runs queue and resume;
each pass routes to the right-sized cloud model; and an opt-in background bible
update runs through Claude's asynchronous Batch API (half price, reconciled
across sessions). The editor gains multi-reviewer annotation triage, a
decluttered run-action palette, a richer reference-file pane, and a per-project
spell dictionary — and, on the safety side, you can browse and restore local
backups in-app.

### Added

- **Semantic "find similar scenes"** — search the manuscript by meaning over the
  on-device embedding index and jump to the scenes closest to a passage or query,
  not just ones that share the same words.
- **Echo / near-duplicate detection** — surface passages that read too much like
  each other (unintended repetition, a description reused across chapters) as
  findings the author can review and revise.
- **Motif & character co-occurrence report** — a report over the index that shows
  which motifs and characters recur and where they appear together, as an
  analysis surface — never a rewrite.
- **Author-maintained research notebook** — keep an author-written research
  notebook in the project that the bibles cite, so tracked facts trace back to a
  source the author controls.
- **Queued, resumable multi-pass AI runs** — run several AI passes as a durable
  queue with per-pass progress that survives a restart, so a long batch of passes
  resumes where it left off instead of starting over.
- **Opt-in background bible update via the Batch API** — an explicit, opt-in
  action rebuilds the bibles from the full manuscript through Claude's
  asynchronous Message Batches API (about 50% cheaper), returning immediately and
  reconciling the finished result out-of-band on a later launch or idle tick.
  Off by default; gated on the `[ai] background-batch-update` flag and a ready
  cloud backend.
- **Cross-pass consensus and span-anchored dismissals for findings** — findings
  corroborated across passes are marked as consensus, and dismissing a finding
  persists against the anchored span so it stays dismissed across re-runs.
- **Multi-reviewer annotation triage** — sort annotations, filter by reviewer or
  by status, and jump to the next open one, so a manuscript marked up by several
  reviewers is workable at a glance.
- **Per-project spell dictionary** — the project's custom spell dictionary is now
  viewable and editable in-app, so book-specific names and terms are managed in
  one place.
- **Browse and restore local backups in-app** — list the local backup archives,
  inspect them, and restore one without leaving Quill; opening a project with a
  newer backup on disk offers to restore it.

### Changed

- **Retrieval-backed continuity and setting passes** — the incremental continuity
  and setting passes pull the most relevant nearby prose from the embedding index
  as context instead of a fixed window, so their observations are grounded in the
  passages that actually bear on the check.
- **Per-question retrieval breadth for grounded Ask** — a grounded-Ask question
  can choose how widely to retrieve, trading focus for coverage per question.
- **Per-pass model routing** — each AI pass routes to the right-sized cloud model
  (a heavier model where it earns its keep, a lighter one elsewhere), rather than
  one model for everything.
- **Reference-file pane improvements** — the reference-file pane gains grouping,
  mouse focus, a resizable divider, and Vim-style close, making a large set of
  reference files easier to navigate.
- **Wheel scroll re-reads earlier prose** — scrolling the wheel up now pages back
  through earlier prose in a chapter rather than stopping at the viewport top.
- **Run-action palette declutter** — the Ctrl-K run-action palette hides actions
  that don't apply in the current context, and remembers group collapse state
  (staying collapsed, and reopening collapsed) between sessions.
- **Internal quality pass** — four oversized modules were split into cohesive
  submodules (`app/vcs_actions`, `export/exporting/manuscript`, `export/pdf/mod`,
  `export/epub/tests`).

### Fixed

- **"Refresh all bibles" runs synchronously** — the interactive "refresh all
  bibles" action now runs synchronously and no longer reports a false batch
  "timed out"; a too-complex character schema that could break the refresh was
  also fixed.
- **Pass-queue resume under `--features local`** — resuming the multi-pass queue
  no longer fails when built with the `local` feature.

[0.8.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.8.0
[0.7.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.7.0
[0.6.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.6.0

## [0.5.0] - 2026-07-07

A collaboration- and safety-focused release. It closes the editorial loop —
import a marked-up review copy so an editor's Word comments arrive as live
annotations, then reply to and resolve those annotation threads without leaving
Quill — and it takes the fear out of losing work: opt-in automatic backup to a
Git remote on a cadence, opt-in scheduled local archives with retention, and
automatic recovery of a stale project lock. Around that, the editor gains
typeset scene-break dividers and regex in project-wide find, the offline AI
gives a real answer on oversized full-book scans, and a documentation and
tech-debt pass lands an authoritative configuration reference and splits the
largest modules.

### Added

- **Import a marked-up review copy** — bring an editor's marked-up `.docx`
  (the review copy Quill exports) back into the project so their Word comments
  land as inline annotations anchored to the exact phrase, closing the
  round-trip from export to editorial feedback.
- **Annotation threads** — reply to an annotation and resolve it, so editorial
  back-and-forth lives on the annotation itself rather than in a separate tool.
- **Automatic backup to a Git remote** — opt in and Quill pushes the project to
  a configured remote on a cadence in the background, so a working copy lives
  off-machine without a manual step.
- **Scheduled local backups** — opt in to automatic local archives on a cadence,
  keeping the N most recent and pruning the rest.
- **Automatic stale-lock recovery** — a project lock left behind by a crashed or
  killed session is detected as stale and reclaimed automatically, instead of
  forcing a manual unlock.
- **Typeset scene-break dividers** — a scene break renders as a centered
  divider in the editor (revealed as raw marks when the cursor is on the line),
  with the following paragraph set flush-left after a break or a code block.
- **Regex in project-wide find** — project-wide find gains regular-expression
  search, at parity with chapter find.
- **Authoritative configuration reference** — `docs/configuration.md` documents
  every configuration key, with a drift-guard that keeps it in sync with the
  code.

### Changed

- **Oversized offline full-book scans fold chapter-by-chapter** — when an
  offline model can't fit the whole book, the full-book scan now folds the
  analysis chapter by chapter on-device (or refuses with a clear message)
  instead of failing with an opaque error.
- **Configuration types consolidated** — the `[ai]` and `[image]` config data
  moved into `config/`, single-sourcing the configuration model.
- **Internal quality pass** — five oversized modules were split into cohesive
  submodules (`project`, `ui/editor`, `buffer/tests`, `export/style`,
  `frontmatter`).

### Fixed

- **Longest-sentences report** — the sentence splitter no longer fuses sentences
  across a scene or paragraph break, and no longer leaks a closing quote onto
  the next sentence.
- **Continuous-view read** — a failed chapter read while reading in the
  continuous view no longer strands the cursor at the boundary; it rewinds
  instead.
- **Onboarding + lock banner** — the read-only lock banner stays visible under
  the first-run onboarding card instead of being hidden by it.
- **Annotation remap** — remapping annotations across an edit burst is now
  linear rather than O(edits × annotations).

[0.5.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.5.0

## [0.4.0] - 2026-07-07

A publishing- and reliability-focused release. It carries a book from draft to a
store-ready print/ebook bundle in one pass (preflight, one-click KDP/IngramSpark,
a print-conformant interior and a wrap cover), sharpens the editor (system
clipboard, mouse selection, more Vim), grows the AI tracking surface within the
assistant boundary (a theme tracker, a manuscript-health dashboard, grounded Ask
over an on-device index), and closes a hardening pass on the write path,
background work, and per-frame cost.

### Added

- **Export preflight (publish health check)** — audits the book against what
  KDP / IngramSpark reject or flag (missing ISBN, no cover or a sub-300-DPI
  cover, no copyright page, an invalid BCP-47 language, bleed set with no
  full-bleed cover, empty chapters, unbalanced emphasis markers) into one
  PASS / WARN / FAIL checklist, saved to `export/preflight-<date>.md`. It only
  validates — it never changes the export.
- **One-click "Prepare for KDP" / "Prepare for IngramSpark"** — sets each
  store's sane defaults (trim, full-bleed allowance, a default language),
  runs the preflight, and — if it passes — emits the print-interior PDF and the
  EPUB together; a failing preflight blocks the export and points at the report.
- **PDF wrap-cover generator** — a single press-ready cover sheet (back + spine
  + front) sized to `2×trim + spine + bleed`, with the spine width computed from
  the interior page count × paper caliper, a reserved barcode/ISBN box, and fold
  guides; written as a PDF/X-3 grayscale file (`export/<book>.cover.pdf`).
- **PDF/X-3:2003 print conformance** — the print interior now carries the
  output-intent, metadata, and color/transparency invariants a print-on-demand
  service expects.
- **Review-copy DOCX export** — a `.docx` built to be marked up: every line
  numbered down the margin, generous 2-inch note margins, and — uniquely among
  the exports — your inline annotations rendered as Word comments anchored to
  the exact phrase.
- **Half-title and "also in this series" front matter** — `half_title = true`
  in `[style]` adds a bare half-title leaf ahead of the title page; a book
  exported from a multi-book series gets an auto-generated "also in this series"
  page (on by default; `series_list = false` to suppress).
- **EPUB image alt text** — author-supplied `*_alt` keys emit as each image's
  `alt`; an image with no alt is declared decorative (`role="presentation"`),
  and the cover always carries a description.
- **Per-book series publishing metadata** — each book in a `[[books]]` series
  gets its own title, subtitle, author, ISBN, and the rest, so an omnibus and
  each volume export with the right identifiers.
- **System clipboard copy/paste** — a Vim yank/delete and a Visual copy place
  the text on the OS clipboard (via OSC 52, so it works over SSH and tmux), and
  your terminal's paste inserts the clipboard at the cursor as one undoable edit.
- **Mouse drag-to-select** — press and drag the left button to make a Visual
  selection (correct across the centered margin, soft-wrap, paragraph indent,
  and fenced-code rows); a plain click still just positions the cursor.
- **Paragraph and sentence Vim motions and text objects** — `}` / `{` jump by
  paragraph, and the prose text objects `ip` / `ap` (paragraph) and
  `is` / `as` (sentence) compose with `d` / `c` / `y`.
- **Vim marks and a jump-back list** — `m{letter}` sets a global (chapter-aware)
  mark, `` `{letter} `` / `'{letter}` jumps to it, and the big jumps (outline,
  corkboard, find/search, go-to-chapter) now record where you left so ``` `` ```
  / `''` pops back a step at a time.
- **`:{N}` line jump and `:%s///` substitute ex-commands** — a bare number jumps
  to that line, and `:%s/old/new/` runs a document-wide substitution.
- **Type-to-filter on the review overlays** — `/` narrows the findings, history,
  annotations, and outline lists as you type.
- **Selected word/char count in Visual mode** — the status bar shows the size of
  the current Visual selection.
- **Snippet caret placeholder** — a `$0` in a snippet sets where the cursor lands
  after insertion (`$$0` for a literal `$0`).
- **Theme/motif tracker** — a fifth incremental story bible that tracks a book's
  themes and motifs and where they recur, the same track-only way as the
  character, timeline, setting, and plot-thread bibles.
- **Manuscript-health dashboard** — a one-screen overview of the open findings,
  now carrying a `kind` field so they group by category (continuity,
  consistency, style, and the rest).
- **On-device embedding index for grounded Ask** — an on-device semantic index
  retrieves the most relevant chapters for an "ask the manuscript" question, so
  grounded Q&A cites the right passages without a cloud round-trip.
- **Export the findings list + re-check all open findings** — write the current
  findings to a file, and re-run every open finding in one pass.
- **Bible-entry provenance line** — each AI-generated bible entry carries a
  readable note of the pass and revision that produced it.
- **Local-model download progress** — downloading an on-device model shows live
  progress inside Quill's own UI instead of a silent wait.
- **Named manuscript milestones + whole-project restore/diff** — name a point in
  the project's history, then restore the whole project to it or diff against it.
- **Deadline-aware goals + completion projection** — set a target date and the
  progress report projects your finish date and the daily pace it implies.
- **Calendar / per-session writing analytics** — a month calendar heatmap of
  daily activity and per-session writing records (words, time, words-per-minute).
- **ODT manuscript import** — import an OpenDocument Text (`.odt`) manuscript
  into a new chaptered project, alongside the existing Markdown / DOCX /
  Scrivener importers.
- **First-run onboarding / quick-start card** — a dismissible card on first
  launch that points at the core keys and next steps.

### Changed

- **Save cost is now independent of project size** — a save touches only the
  chapter(s) being written, so it stays flat whether the project is one chapter
  or a series of thousands.
- **Live word count is maintained incrementally** — each edit adjusts the count
  by re-segmenting only the touched line(s) rather than recounting the whole
  document every frame, and the cold rebuild no longer materializes the whole
  document into one string.
- **Per-frame and idle allocation cleanups** — the conceal list, header/epigraph
  blocks, and other per-draw work are cached and reused across unchanged frames,
  and the idle git-status walk backs off when nothing changes.
- **A dead VCS worker surfaces its real cause** instead of a generic failure
  message.
- **Internal quality pass** — the no-panic policy holds (bare `unwrap` / `expect`
  stay `deny` in production code) and the large `app/worker.rs` was split into
  cohesive submodules to stay under the file-size budget.

### Fixed

- **Symlink-safe atomic save, a multi-process instance lock, and a read-error
  distinction** — an atomic save through a symlinked chapter now replaces the
  link's real target instead of orphaning it; an advisory `.quill/lock` stops two
  Quill processes from racing writes (a live holder opens read-only); and a hard
  read error is distinguished from an empty/missing file, so a transient failure
  no longer zero-word-caches a chapter or over-offers recovery.
- **Stale-context guards on background results** — a `git pull` that lands after
  you've navigated away or typed no longer clobbers unsaved edits; switching
  books is refused while an AI pass is in flight (so its result files under the
  right book); and quitting with an AI pass, push/pull, or model download in
  flight warns first instead of silently dropping the work.
- **Front-matter round-trip edge cases** — a duplicate key and a bare `CR` line
  ending in a front-matter block now round-trip correctly.
- **First-line paragraph indent no longer overflows the wrap column.**
- A batch of reliability and performance quick-wins from the audit.

[0.4.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.4.0

## [0.3.0] - 2026-07-03

A large release across the editor, publishing exports, AI assistance, and
project management, plus a round of reliability and internal-quality work.

### Added

- **Mouse support** — click to position the cursor and scroll with the wheel,
  alongside the existing keyboard-first bindings.
- **Corkboard** — an index-card overview of a chapter's scenes (one card per
  heading), navigable and editable: reorder scenes (heading and prose move
  together), cycle a scene's status, and write a one-line synopsis, all kept in
  front matter. A **manuscript-wide corkboard** spans every chapter of the book.
- **Outline jump-to-heading** — the `Ctrl-O` outline is navigable; pick a
  heading to jump the editor there.
- **First-line paragraph indent** — an opt-in editor display mode that indents
  the first line of each prose paragraph, matching how the designed exports look.
- **Fenced-code preview** — ` ``` ` blocks render as a set-apart, tinted box
  with the fences hidden, revealing the raw markup only when the cursor is
  inside.
- **Resume last position** on open, and **go to a chapter** by number or title
  without scrolling the browser; opt into reopening the last project on a bare
  launch with `[projects] open_last`.
- **Spell suggestions** — pick an offline correction for a flagged word
  (`z=` / `Alt-=`).
- **Continuous-view undo/redo** and Vim operators, find, and range edits now
  work across the continuous manuscript.
- **Modeless editing keys** — forward-delete, word-delete, word motion, and
  paging in the default (non-Vim) insert mode.
- **In-app series management** — create a series, add / remove / reorder books,
  and switch the active book from within Quill (`[[books]]` manifest).
- **Import preview** — a preview / mapping step before an import writes
  chapters, with a report of any footnotes, images, or tables dropped.
- **New-project structure templates** offered when creating a project.
- **Project browser** — filter, rename, and delete projects from the library.
- **Story bible export** to one clean document, plus a **Browse story bible**
  editor, a **plot-thread / arc tracker** as a fourth incremental bible, an
  **AI name generator**, a **character-relationship index**, and a **thesaurus**
  that picks and places a chosen candidate.
- **Grounded Ask** — jumpable chapter citations and a persisted session Q&A
  history; **re-check** a single finding or a plot dangling-thread scoped to its
  chapters; a **bible delta view** after a clean revise; and a **Refresh all
  bibles** command with one combined cost confirmation.
- **In-app on-ramp to enable AI** (and a plain offline **Settings** menu).
- **On-device local models** on candle 0.11 with the **Qwen3** dense family,
  memory-aware model selection, and a **Local AI models** manager to download,
  choose, and remove models.
- **Print PDF** front matter numbered in Roman with the body restarting at
  Arabic 1, a **printed Contents page**, **cover bleed** / full-bleed cover, and
  `/TrimBox` / `/BleedBox` / `/Info` on print PDFs.
- **EPUB** accessibility metadata (`schema:*`), EPUB 3 landmarks and
  `epub:type` semantics (including role-specific types), and embedded-font
  subsetting.
- **DOCX** running header, page-number field, and a TOC field.
- **New section roles** — dedication, also-by, about-the-author, colophon — and
  a per-book copyright page reprinted in a series omnibus.
- **Offline project archive & restore** to a single `.zip`, and **save reports
  to a file** (progress and analysis snapshots).
- **Per-project config** — override any global setting for a single book under a
  `[quill]` table in `book.toml`.
- **Ctrl-C to quit** (with the same unsaved-changes guard as `Ctrl-Q`).
- **Pre-commit diff / status review** before committing to version history, and
  in-app resolution of a diverged branch on pull.

### Changed

- **Justified body text with hyphenation** and widow/orphan control in the
  designed exports.
- **Smaller, correct exports** — embedded font and content streams compressed
  with FlateDecode, PDF images compressed with a low-DPI cover warning, the
  `/ToUnicode` CMap compressed, EPUB fonts subsetted, and multi-style
  (emphasis) font resolution and embedding fixed.
- **More accurate AI cost estimates**, single-sourced Claude rates from a model
  catalog, and a lazy, memoized keychain read for the API key.
- **Internal quality pass** — a curated clippy lint set promoted from warn to
  deny (including a no-panic policy: bare `unwrap`/`expect` are denied in
  production code), and large modules split to stay under the size budget.

### Fixed

- Guard `git pull` against clobbering a dirty working tree, and prune deleted
  chapters from a series' incremental bible state.
- Extend crash recovery to the continuous view, route AI-artifact and recovery
  writes through the atomic layer, and abort writes derived from a failed read.
- Harden archive restore against zip bombs and OOM, tolerate CRLF front-matter
  fences, and stop the setup wizard and notes-import from clobbering existing
  files.
- Sync the book title when renaming a project, and keep a focused outline from a
  stale, out-of-range selection.

[0.3.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.3.0

## [0.2.0] - 2026-06-29

The first version bump since the initial `0.1.0`, collecting the user-facing
features that have landed since.

### Added

- **Export to EPUB** with a `[style]` styling seam: scene-break/title/part
  ornaments, body and heading font selection, chapter-heading treatments, drop
  caps, and image ornaments.
- **Export to PDF**, including a print-ready interior for print-on-demand,
  `[style]` support, image ornaments, and full-Unicode text via embedded Type0
  fonts.
- **Export style presets** — a preset library plus a pre-export style preview.
- **Import a Word (.docx) manuscript** into a project.
- **Import a Scrivener (.scriv) project** into a project.
- **Author-defined text snippets** for fixed insertions.
- **Rename a project's folder** from within Quill.
- **Bare launch** names a new project in the projects home rather than the
  current working directory.

### Changed

- Tree-wide `rustfmt` adoption with documented conventions and a blame-ignore
  revision list.
- A round of tech-debt cleanups: de-duplicated AI pass machinery, single-source
  struct/enum/theme/config tables, shared serializers, and faster history
  persistence.

[0.2.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.2.0
