# Progress reports & writing goals

Quill can produce a single-screen status report for a project, tracking the
manuscript against author-set goals and showing daily momentum. This document
specifies the goal metadata, the tracking data Quill must keep, and the exact
formula behind every line of the report.

## Sample report

```
+--------------------
| Red State Earth
|
| story :   163,204 / 150,000  words ( 108.80 % )
|           652 / 600  pages
|           56 / 60 chapters ( 93.33 % )
|
|           expansion ratio: 1.17
|           words at this ratio: 174,861
|           pages at this ratio: 699
|
|           163,204 / 174,861 words at this ratio ( 93.33 % )
|           652 / 699 pages at this ratio
|
| xxx:               12
| moved to unused:   0
|
| today's word delta: +2,860   (+3,143 -283)
| part deltas:
|   The Reckoning   55,501 words   -4      (+12 -16)
|   Part 2          61,240 words   +1,327  (+1,486 -159)
|   Part 3          28,110 words   +521    (+627 -106)
|   Part 4          18,353 words   +1,016  (+1,018 -2)
| days since start:  156
| average rate:      1,046 words / day
| to go:             0 words ; 0 days ( 16 Jun 2026 )
| to go (expansion): 11,657 words ; 11 days ( 27 Jun 2026 )
+--------------------
```

## Goal metadata

Goals live in the **project manifest** (the root file that records the
chapter/book/act tree; see the README "Project structure" section) so they
travel with the manuscript and stay human-readable. Word, page, and chapter
counts aggregate across the project's per-chapter files. Pages are always
*derived* from words via `words_per_page`, never tracked directly.

```yaml
---
title: Red State Earth
start_date: 2026-01-11        # anchors "days since start" and rate
goals:
  words: 150000              # target word count
  chapters: 60              # target chapter count
  words_per_page: 250        # pages = words / words_per_page
markers:
  todo: xxx                 # token counted on the "xxx:" line (case-insensitive)
unused_heading: Unused        # chapters/sections under this heading are excluded
                              # from totals; words moved in today feed
                              # "moved to unused"
part_label: part              # the displayed term for a grouping; default "part"
                              # (set to "act", "book", … for the report header
                              # and auto-named parts)
parts:                        # optional: group chapters into parts for part deltas
  - { name: "The Reckoning", chapters: [1, 14] }   # inclusive chapter ranges
  - { chapters: [15, 32] }                          # name optional → "Part 2"
  - { chapters: [33, 46] }
  - { chapters: [47, 60] }
---
```

Parts are an optional grouping layer **between book and chapter** (see the
structure tree in the README). Each part is an inclusive, 1-based chapter range
with an optional `name`; an unnamed part is shown as `"{part_label} {n}"` (e.g.
"Part 2"). The collective term is configurable via `part_label` (default
`part`), so an author can use "act", "book", or whatever fits. If `parts` is
omitted, the "part deltas" block is simply not rendered.

## Tracked data

To compute deltas and rates, Quill keeps a per-day snapshot log (append-only,
`.quill/history.jsonl` inside the project). One record is written per report/day
(re-written in place if it already exists, so there's one line per day), with
the running edit counters reset at the day boundary. The boundary is the
author's *local* midnight: `date::today()` reads `OffsetDateTime::now_local()`
(via the `time` crate), falling back to UTC if the platform won't yield a local
offset.

```jsonc
{
  "date": "2026-06-16",
  "words": 163204,            // total manuscript words (excludes unused)
  "added": 3143,             // words inserted today
  "removed": 283,            // words deleted today
  "moved_to_unused": 0,       // words relocated out of the manuscript today
  "todo": 12,                 // count of TODO markers in the manuscript
  "parts":       [55501, 61240, 28110, 18353],  // per-part words, manifest order
  "part_added":  [12, 1486, 627, 1018],          // per-part words inserted today
  "part_removed": [16, 159, 106, 2]              // per-part words deleted today
}
```

- `added`/`removed` are accumulated live as the author types: the idle handler
  (after each input burst) re-tallies the project word count and books the
  signed delta against a running baseline, so keystrokes stay record-only.
- `moved_to_unused` is, for now, the words removed by **deleting a chapter** —
  structural drops are booked here rather than as `removed`, and the baseline is
  reset so the deletion isn't counted as prose loss. The `unused` *heading*
  exclusion (moving sections under it) lands with #53.
- The per-part fields (`parts`, `part_added`, `part_removed`) carry the same
  numbers split by part (manifest order), attributing each edit to the part of
  the chapter it occurred in. They're omitted from the JSON when the manifest
  defines no parts, and are resumed on reopen only when their length still
  matches the current parts config.
- `words`, `todo`, and the per-part `parts` totals are measured from the buffer
  at report time.
- "today" = changes since the most recent prior snapshot (or `start_date` on day
  one).

## Field formulas

Let `W`, `C` = current words / chapters; `Wt`, `Ct`, `wpp` = goal words /
chapters / words-per-page; pages use `floor(words / wpp)`.

**Book sections (#182):** `W` and `C` count **body chapters only** — front- and
back-matter sections (a `role:` in a chapter's front matter, e.g. `prologue`,
`appendix`, `glossary`; see [book sections](../README.md#keys)) are excluded from
the goal, the projection, and the rate, so the target tracks the novel rather
than being inflated by a long introduction or appendix. Their combined word count
is shown on its own line (`(+N words in front/back matter, not counted)`) so
nothing is hidden. An all-body project is unaffected (the line is omitted).

| Report line | Formula |
| --- | --- |
| `words ( % )` | `W / Wt`, percent = `W / Wt * 100` |
| `pages` | `floor(W/wpp) / floor(Wt/wpp)` |
| `chapters ( % )` | `C / Ct`, percent = `C / Ct * 100` |
| completion fraction `f` | `C / Ct` |
| projected words `Wp` | `W / f` *(extrapolate current pace of prose-per-chapter to all chapters)* |
| `expansion ratio` | `Wp / Wt`, shown to 2 dp |
| `words at this ratio` | `Wp` (rounded) |
| `pages at this ratio` | `floor(Wp / wpp)` |
| `W / Wp words at this ratio ( % )` | `W / Wp * 100` (equals `f` by construction) |
| `xxx` | count of the configured TODO marker |
| `moved to unused` | today's `moved_to_unused` |
| `today's word delta` | net `added - removed`, with `(+added -removed)` shown |
| `{part_label} deltas` | per-part name + current words + net `part_added[n] - part_removed[n]` with adds/dels |
| `days since start` | `report_date - start_date`, in days |
| `average rate` | `floor(W / days_since_start)` words/day |
| `to go` | words `max(0, Wt - W)`; days `round(words / rate)`; ETA `report_date + days` |
| `to go (expansion)` | words `Wp - W`; days `round(words / rate)`; ETA `report_date + days` |

Numbers are grouped with thousands separators; deltas are signed (`+`/`-`) and
color-coded (green for gains, red for losses) in the TUI.

## Open questions

- **Pace basis for projection.** The sample extrapolates by *chapter*
  completion (`W / (C/Ct)`). An alternative is a manually pinned expansion ratio
  the author sets directly. Support both — auto-derive by default, allow an
  override in `goals`.
- **Part attribution of edits** is by the part of the *chapter* being edited
  (per-chapter word baselines, retallied on idle). In the continuous view this
  settles at save granularity; finer cursor-position attribution within a
  chapter would need the structure parse to be current.
- **Day boundary / timezone** for resetting daily counters — resolved: use the
  author's local date (`time::OffsetDateTime::now_local`, UTC fallback). A
  configurable "my day ends at 3am" offset is deferred until someone asks; the
  local midnight is the sensible default.
