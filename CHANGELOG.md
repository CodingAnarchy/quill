# Changelog

All notable changes to Quill are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
