# Performance: benchmarks, corpus, and targets

Quill must stay instant on real manuscripts — typing, scrolling, chapter
navigation, and saving snappy from a short story up to a multi-book series, not
just on toy files (epic #14). Responsiveness is hard to retrofit and easy to
regress, so this is the measured gate that catches a slowdown before an author
hits it mid-draft. You can't defend a target you don't measure.

## Running the suite

```sh
cargo perf   # alias for: cargo test --release -- --ignored --nocapture --test-threads=1
```

The benchmarks are `#[ignore]`d so they don't run under plain `cargo test`:
they measure wall-clock time, which is only meaningful in a release build (a
debug build is 3–18× slower and the thresholds false-fail). They also run
**single-threaded** (`--test-threads=1`): several guards build and read
series-sized projects (thousands of files), and running them concurrently with
the CPU-bound timing benchmarks (reparse, render, keystroke) would starve those
of a core and false-fail their sub-frame thresholds. Run `cargo perf`
**before every release**, and whenever you touch the parse, the edit path,
rendering, or project load. `--nocapture` prints each measured time next to its
target.

Each benchmark lives next to the code it guards (so a regression points at the
cause) and asserts against a written-down budget — it *fails* if exceeded,
rather than just reporting. This file is the index; the budgets and rationale
live here so a contributor can defend or update a number.

## The corpus

Quill never holds the whole novel in a buffer — one chapter in the
single-chapter view, at most ~3 (a window around the edit point) in the
continuous view — so per-buffer cost is bounded to the editing window. The
benchmarks exercise the three shapes that matter:

- **A long single chapter** — the largest buffer the single-chapter view holds.
- **A ~3-chapter window** — the continuous view's realistic worst case.
- **A series-sized project** — many chapter *files* (only a window is ever
  loaded), for open and navigation cost as the project grows.

Corpora are generated deterministically in-test, so the suite needs no fixtures.

## The benchmarks

| Benchmark | Location | Corpus | Budget | What it guards |
| --- | --- | --- | --- | --- |
| `keystroke_is_decoupled_from_document_size` | `buffer.rs` | 150-chapter buffer (~75k words) | < 4 ms / keystroke | A keystroke only *records* the edit (the reparse is deferred to idle), so its cost is in the edit, not the document size. |
| `reparse_is_fast_at_window_size` | `buffer.rs` | ~3-chapter window (~4.5k words) | < 16 ms (best of 5) | A full incremental reparse on the continuous-view window stays under one frame. Best-of-5 because a single sample catches one-off noise (scheduling, CPU ramp); noise only ever *adds* time. |
| `render_stays_within_a_frame` | `ui/mod.rs` | ~82k-word chapter + 150-chapter outline pane | < 16 ms / frame | A full `draw()` stays under one frame. The status bar's word count is O(document); the pane reads cached per-chapter metadata, never files. Catches rendering that re-reads files or does per-chapter work proportional to the novel. |
| `open_is_fast_for_a_large_novel` | `project.rs` | 150 chapters on disk | < 250 ms | Opening reads and parses each chapter once (word count + front matter) and caches it; catches a regression to per-chapter re-reads or O(n²) work at open. (Typically a few ms.) |
| `open_is_fast_for_a_series_sized_project` | `project.rs` | 1500 chapters, one book | < 500 ms | Open scales linearly with the project (the per-chapter cache read is the only proportional work, and it's bounded). (Typically ~25–60 ms.) |
| `open_is_fast_for_a_multi_book_series` | `project.rs` | 1500 chapters across 6 books (the `[[books]]` format, #328) | < 500 ms | Opening the multi-book manifest runs the books-array parse and the active-book hoist, then caches only the *active* book's chapters; catches a regression that eagerly reads every book's files at open. (Typically ~10 ms — faster than the single-book guard, since only one book is cached.) |
| `navigation_is_fast_in_a_series_sized_project` | `app/tests/chapters.rs` | 600 chapters on disk | < 10 ms / switch | Switching chapters reads/parses one chapter file — never work proportional to the project. (Typically ~0.5 ms.) |
| `switching_books_is_fast_in_a_series` | `project.rs` | 2 books × 750 chapters (#329) | < 500 ms | Switching the active book caches the target book's chapters once (the bounded work `open` does for one book) — never the whole series. (Typically ~10 ms.) |
| `tree_rows_are_fast_for_a_series` | `app/tests/books.rs` | 4 books × 200 chapters, all expanded (#330) | < 16 ms | Building the tree navigator's visible rows reads only cached per-chapter metadata (titles, roles), never files — so even fully expanded it stays within a frame. (Typically ~70 µs.) |
| `tally_is_independent_of_project_size` | `app/tests/chapters.rs` | 2000 chapters on disk | < 30 µs / idle tick | The daily word tally touches only the chapters that changed (the live active one, plus any whose cache moved), not all of them. A regression to the per-chapter walk would be ~50–100 µs here. (Typically ~6 µs.) |

Budgets are deliberately generous headroom over the typical measured time: they
catch *regressions* (a gross slowdown), not micro-fluctuations. When an
optimization lands (#218–#220), tighten the relevant budget toward the new
typical so it keeps defending the win.

## The invariants these protect

- **Parsing is off the keystroke path** — record the edit on keypress, reparse
  on idle (the `main.rs` event loop). The keystroke guard enforces this.
- **Rendering never touches the filesystem** — file-derived data (word counts,
  front-matter metadata) is cached on `ChapterEntry` at open and kept current as
  the author edits. The render and navigation guards enforce this.
- **Work stays incremental** — editing one chapter doesn't reprocess the whole
  project (parse, word counts, AI caches). The navigation guard, and the
  word-count work in #220, enforce this.
- **Memory is bounded to the editing window** — the project holds one small
  `ChapterEntry` per chapter (title, file, cached count, front-matter metadata),
  never a chapter's body text, and the editor holds one live `Buffer` (the active
  chapter) — at most ~3 in the continuous view. So a series-sized project's
  footprint grows with the *number* of chapters (small metadata), not with the
  manuscript's word count. Opening or editing one chapter reads/parses only that
  chapter's file, never the others'.
