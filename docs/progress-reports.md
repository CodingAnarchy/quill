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
| act deltas:
|   1: -4    (+12 -16)
|   2: +1,327  (+1,486 -159)
|   3: +521  (+627 -106)
|   4: +1,016  (+1,018 -2)
| days since start:  157
| average rate:      1,039 words / day
| to go:             0 words ; 0 days ( 16 Jun 2026 )
| to go (expansion): 11,657 words ; 11 days ( 27 Jun 2026 )
+--------------------
```

## Goal metadata

Goals live in the project's front matter so they travel with the manuscript and
stay human-readable. Pages are always *derived* from words via `words_per_page`,
never tracked directly.

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
acts:                         # optional: group chapters into acts for act deltas
  - { name: "Act I",   chapters: [1, 14] }   # inclusive chapter ranges
  - { name: "Act II",  chapters: [15, 32] }
  - { name: "Act III", chapters: [33, 46] }
  - { name: "Act IV",  chapters: [47, 60] }
---
```

Acts are an optional grouping layer **between book and chapter** (see the
structure tree in the README). If `acts` is omitted, the "act deltas" block is
simply not rendered.

## Tracked data

To compute deltas and rates, Quill keeps a per-day snapshot log (append-only,
e.g. `.quill/history.jsonl` inside the project). One record is written per
report/day, with the running edit counters reset at the local day boundary.

```jsonc
{
  "date": "2026-06-16",
  "words": 163204,            // total manuscript words (excludes unused)
  "acts": [55501, 61240, 28110, 18353],   // words per act, same order as goals.acts
  "added": 3143,             // words inserted today
  "removed": 283,            // words deleted today
  "act_added":   [12, 1486, 627, 1018],
  "act_removed": [16, 159, 106, 2],
  "moved_to_unused": 0,       // words relocated into the unused heading today
  "xxx": 12                  // count of TODO markers in the manuscript
}
```

- `added`/`removed` are accumulated live as the author types (insert and delete
  events tally word-boundary changes), attributed to the act the edit occurred
  in.
- `words` and `xxx` are measured from the buffer at report time.
- "today" = changes since the most recent prior snapshot (or `start_date` on day
  one).

## Field formulas

Let `W`, `C` = current words / chapters; `Wt`, `Ct`, `wpp` = goal words /
chapters / words-per-page; pages use `floor(words / wpp)`.

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
| `act deltas: n` | per-act `act_added[n] - act_removed[n]` with adds/dels |
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
- **Act attribution of edits** depends on the structure parse (tree-sitter
  outline) being current; recompute the active act on cursor move.
- **Day boundary / timezone** for resetting daily counters — use the local
  date; make it configurable for writers who work past midnight.
