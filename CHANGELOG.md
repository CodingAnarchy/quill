# Changelog

All notable changes to Quill are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
