# Quill — working notes for Claude

Quill is a terminal editor optimized for writing novels (Rust, `ratatui` +
`crossterm`, rope-backed via `ropey`, Markdown parsed incrementally with
`tree-sitter-md`). See `README.md` for the product roadmap and `docs/` for the
feature specs. Storage model: one chapter per file; a buffer holds one chapter
(single-chapter view) or at most ~3 chapters (continuous view), never the whole
novel.

Chapter files may begin with a `---`-fenced YAML front matter block. Quill
splits it off when loading (`src/frontmatter.rs`), keeps it out of the prose
buffer so the editor shows only the body, and re-attaches it verbatim on save.
Titles stay in the manifest; front matter carries the rest of the chapter
metadata (the #3 epic builds typed access on this).

The continuous-manuscript view (`Ctrl-l`) concatenates a window of up to ~3
adjacent chapters into one editable buffer (`src/manuscript.rs`), paging
chapters in/out around the cursor and writing edits back to each chapter's file.
Chapter boundaries are fixed seams (editing never merges chapters); the centered
title dividers between chapters are render-only.

## AI assistance boundaries (non-negotiable)

Quill's AI is an **assistant, not a co-writer**. It must **never generate or
suggest manuscript prose** — no drafting, rewriting, tightening, expanding, or
"here's how I'd word it" suggestions, and no suggesting larger edits to the
author's text.

AI features are limited to:

- **Structure** — outlines, beat sheets, scene notes (notes, never written
  prose).
- **Tracking** — the human-readable story bible (characters, timeline, setting).
- **Markers** — inserting `xxx` TODO notes the author can act on.
- **Analysis** — flags and findings (continuity, consistency, style *observations*),
  surfaced for the author to fix themselves; never an edited replacement.
- **Reference lookup** — a thesaurus / word search the author *deliberately*
  invokes for a specific word or phrase, returning candidate words (or short
  phrases) the author chooses among. The lookup may read the surrounding line to
  judge which candidates *fit*, but it returns only options for the requested
  slot — it never rewrites that sentence, edits any other prose, or produces
  unsolicited text. The author picks and places the word; Quill doesn't.

When adding or changing an AI feature, hold it to this line: it may *track*,
*outline*, *mark*, *observe*, and *look up*, but the prose is the author's. A
feature that would draft or reword manuscript text is out of scope — surface the
issue, don't write the fix. (This retired the rewrite/tighten/expand "selection
assistant"; off-manuscript artifacts like a synopsis or query letter are the only
prose the AI produces, and never inside the manuscript.)

The line between *reference lookup* and *co-writing*: answering "a two-syllable,
common word for X to end 'she was feeling…'" with a list of words is reference —
the author asked for that one slot and chooses the result. Volunteering a rewrite
of the sentence, suggesting edits to a passage the author didn't single out, or
returning a drafted scene is co-writing, and out of scope. Context informs *fit*;
it never licenses editing the surrounding prose.

## Code conventions

- **Formatting is `rustfmt`'s** (#459). Run `cargo fmt` before committing and
  keep `cargo fmt --check` clean; don't hand-format against the tool or
  reintroduce the old compact style. The one-time tree-wide reformat is listed
  in `.git-blame-ignore-revs` — run `git config blame.ignoreRevsFile
  .git-blame-ignore-revs` once so `git blame` skips it.
- **`use` imports go at the top of the module**, after the module doc comment —
  not inside functions or partway down the file. Test modules follow the same
  rule: put the test `use`s at the top of the `#[cfg(test)] mod tests` block,
  not inside individual `#[test]` fns. The only imports that stay inside a fn
  are the ones a module-level import would break or leave unused — a
  `#[cfg(unix)]` or feature-gated item, or a `#[cfg(test)]` test seam in an
  otherwise non-test module — and each carries a comment saying why (#1189).
- **Prefer a `use` import over spelling out a full module path** at the call
  site. Write `use std::fs;` then `fs::write(...)`, or `use crate::buffer::count_words;`
  then `count_words(...)` — not `std::fs::write(...)` / `crate::buffer::count_words(...)`
  inline. (Disambiguate with a partial path or `as` alias only when two names
  would otherwise collide.)
- **Keep source files under ~1,000 lines.** When a module approaches that,
  extract a cohesive group of types/traits into its own file (e.g. the editor
  modes out of `app.rs` into a `mode.rs`) rather than letting one file sprawl.
  This counts the whole file; a test module that has grown large is itself a
  signal to split the module.

## Standard checks

Run these before opening a PR:

```sh
cargo fmt --check     # formatting must match rustfmt (run `cargo fmt` to fix)
cargo test            # unit + render tests (must be green)
cargo clippy --all-targets   # must be clean
```

(CI enforces these on every PR — see `.github/workflows/ci.yml`: fmt on ubuntu,
clippy (`-D warnings`) and tests on both ubuntu and macos, plus a
`--features local` job and an informational `cargo perf` run on pushes to
master.)

**The Rust version is pinned in `rust-toolchain.toml`** and that file is the
only place it's written down. rustup reads it for every cargo command here, and
CI installs no toolchain of its own, so the checks above run on the same
compiler locally and on the runners. Don't chase `stable` in either place: it
resolves to "released today" on CI and "whatever you last `rustup update`d to"
locally, and that gap turned a Rust release into red CI on master (#1198).
Upgrading is a deliberate PR — edit `channel`, run the checks, and fix whatever
new lints the release brings.

## Performance guards (ignored tests)

Some tests are `#[ignore]`d because they measure timing and only make sense in
release — in a debug build the unoptimized code is 3–18× slower and the
thresholds false-fail, so they can't run as part of plain `cargo test`. **Run
them whenever you touch the parse, the edit path, rendering, or project load**
— they back the "typing stays instant" promise.

```sh
cargo perf   # alias for: cargo test --release -- --ignored --nocapture
```

What they assert:

- `keystroke_is_decoupled_from_document_size` (`buffer.rs`) — a keystroke only
  marks the parse stale (no parse, no tree bookkeeping), so it stays cheap
  (~µs) even on a ~110k-word buffer of realistic paragraphs.
- `reparse_is_fast_at_window_size` (`buffer.rs`) — on a ~13k-word, 3-chapter
  window, the reparse after a keystroke re-parses only the edited block (see
  `src/syntax/segment.rs`; ~0.2ms, budget 2ms), and the worst case — an
  unclosed fence near the top, re-parsing the rest — stays under a 16ms frame.
- `render_stays_within_a_frame` (`ui/tests/perf.rs`) — a full `draw()` (large
  chapter + a many-chapter outline pane) stays under one frame. The main pass
  renders unfolded, wrapped prose with emphasis on every line plus spell flags,
  proximity repeats, style flags, and annotations, and a second adds an active
  search with thousands of matches; both must stay under 4ms (~0.5ms today), so
  per-character scans of whole-buffer highlight lists (~45ms here) trip it —
  narrow each list to the row by binary search instead. Folded sections, a modal,
  a scroll offset, and the assistant pane are timed against the 16ms frame. The
  status bar's live word count is O(document) per draw; the pane reads cached
  per-chapter metadata, never the files. If rendering ever re-reads files or
  does per-chapter work proportional to the novel, this catches it.
- `open_is_fast_for_a_large_novel` (`project.rs`) — opening reads and parses
  every chapter file once (word count + front matter); even a 150-chapter
  novel opens in a few ms.
- `open_is_fast_for_a_series_sized_project` (`project.rs`) — open scales linearly
  and stays near-instant for a 1500-chapter single-book project (~30–60ms); the
  project holds small per-chapter metadata only, never chapter body text.
- `open_is_fast_for_a_multi_book_series` (`project.rs`) — opening the multi-book
  `[[books]]` format (#328) at series scale (1500 chapters across 6 books) runs
  the books-array parse + active-book hoist and (since #331) caches every book's
  word counts for series rollups, the same total file work as one book of that
  size (~20ms); catches a regression to O(n²) work on the larger manifest.
- `switching_books_is_fast_in_a_series` (`project.rs`) — switching the active
  book of a multi-book series (#329) caches the target book's chapters once
  (the bounded work `open` does for one book), never the whole series (~10ms).
- `navigation_is_fast_in_a_series_sized_project`
  (`app/tests/chapters_counts_and_perf.rs`) — switching chapters in a
  600-chapter (series-sized) project reads/parses one chapter file, never work
  proportional to the project (~0.5ms/switch). Some chapters carry annotations,
  and it asserts a clean switch never rewrites (and fsyncs) `annotations.toml`.
- `tree_rows_are_fast_for_a_series` (`app/tests/books.rs`) — building the series
  tree navigator's rows (#330) reads only cached per-chapter metadata, never
  files; fast (~70µs) even with every book of an 800-chapter series expanded.
- `tally_is_independent_of_project_size` (`app/tests/chapters.rs`) — the daily
  word tally (every idle tick) touches only the chapters that changed, not all of
  them; flat (~6µs) even at 2000 chapters.
- `keystroke_with_settled_pass_stays_within_a_frame`
  (`app/tests/chapters_counts_and_perf.rs`) — a keystroke as the event loop pays
  for it (`on_key` + the per-batch `on_input_settled` pass + the throttled
  recovery tick) on a dirty ~6k-word chapter stays under a frame (~0.2ms since
  the per-block reparse; ~10ms before it). The heavy idle work (spell re-judging, tally, the fsync'd recovery
  write, undo sealing) is debounced to a pause in input (`on_quiet`); running it
  per batch again would roughly double this and trip the guard.
- `spell_pass_is_fast_at_window_size` (`spell.rs`) — the idle spell-check pass
  (#357) over a window-sized chapter stays well within a frame (~0.4ms at 11k
  words), so flagging on idle never stutters. It runs on the idle tick, never the
  keystroke path; the one-time dictionary parse is warmed before timing.

The corpus, every benchmark, its target, and the rationale are written up in
[`docs/performance.md`](docs/performance.md) — the pre-release perf gate. Run
`cargo perf` before a release and whenever you touch the parse, the edit path,
rendering, or project load.

If you make per-keystroke work depend on document size, the first guard will
catch it. Keep parsing off the keystroke path (mark dirty on edit, reparse per
input batch in the `main.rs` event loop), and keep the reparse proportional to
the edit (the per-block parse cache in `src/syntax/segment.rs`). Keep per-frame rendering off the filesystem —
cache file-derived data (word counts, front-matter metadata) on the manifest
at open, like `ChapterEntry` does.

## Live TUI verification (tmux)

Unit tests cover render logic via `ratatui::TestBackend`, but for visual
features (syntax highlighting, soft-rendered emphasis, the outline pane) it's
worth driving the real binary in a PTY and capturing the screen. We use `tmux`
(install once with `brew install tmux`).

`capture-pane -p` gives plain text (proves layout and marker concealment);
`capture-pane -e` keeps the ANSI escapes (proves styling: italic `^[[3m`, bold
`^[[1m`, heading color `^[[38;5;6m`).

```sh
cargo build
# Quill operates on a project directory; seed one with a manifest + chapter.
rm -rf /tmp/quill-sample && mkdir -p /tmp/quill-sample
printf 'title = "Sample"\n\n[[chapters]]\ntitle = "One"\nfile = "one.md"\n' > /tmp/quill-sample/book.toml
printf '# Title\n\nThe night was *dark* and **long**.\n' > /tmp/quill-sample/one.md

tmux kill-session -t quill 2>/dev/null
tmux new-session -d -s quill -x 90 -y 16 "target/debug/quill /tmp/quill-sample"

# Bounded wait for first paint (no foreground `sleep`; poll until content shows).
i=0; until tmux capture-pane -p -t quill 2>/dev/null | grep -q "Title"; do
  i=$((i+1)); [ "$i" -gt 200 ] && { echo TIMEOUT; break; }
done

tmux capture-pane -p -t quill            # readable screenshot
tmux capture-pane -e -p -t quill | sed 's/\x1b/^[/g'   # with styling escapes

# Drive input, then re-capture to verify interactive behavior:
tmux send-keys -t quill C-o              # e.g. toggle the outline
for _ in $(seq 1 6); do tmux send-keys -t quill Down; done
i=0; until tmux capture-pane -p -t quill | grep -q OUTLINE; do
  i=$((i+1)); [ "$i" -gt 200 ] && { echo TIMEOUT; break; }
done
tmux capture-pane -p -t quill

tmux kill-session -t quill               # always clean up
```

Notes:
- Use the bounded `until` poll to wait for paint — `sleep` is blocked in this
  harness, and the editor renders within a few milliseconds anyway.
- Keys: `C-o` toggles the outline, `C-l` toggles the continuous view, `C-s`
  saves, `C-q` quits, `C-n`/`C-p` step chapters (single view), arrows move.
- Always `tmux kill-session -t quill` when done so sessions don't linger.
