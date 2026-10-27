Quill — Export & Publishing

Export is where a finished manuscript becomes something you can hand off, sell, or
hold in your hand. Quill compiles your project — chapters in reading order, parts
and titles, epigraphs, emphasis, and everything the `[style]` and `[print]` tables
describe — and writes it out in whichever format you ask for. Everything happens
locally: no second paid tool, no Mac, no cloud round-trip. Files land under
`export/` in the project directory.

Press **Ctrl-E** to open the export menu. This page walks through the formats,
the design system that shapes the ebook and print interior, the preflight check,
the print-on-demand bundle, and the previews that let you see a look before
committing to a full export.


## The formats

Quill writes six formats. The export menu offers each one directly; for a
multi-book series, each format is offered twice — the whole series in reading
order, or just the active book.

- **Manuscript (Markdown)** — one compiled `.md` file. Chapters in reading order,
  parts and titles as headings, epigraphs set apart, `*`/`**` emphasis preserved.
  This is the hand-off / convert format: feed it to pandoc, paste it into a
  submission form, or keep it as a plain-text archive.

- **Word (.docx)** — a `.docx` in standard manuscript format: a title block, each
  chapter on a fresh page, double-spaced Times New Roman, first-line indents, a
  running header carrying author, title, and a live page number, and `*`/`**`
  emphasis as italic and bold. A live table of contents updates when the reader
  opens the file. This is the format editors and agents expect.

- **Scrivener (.scriv)** — a Scrivener 3 project bundle, not a single file: a
  `.scrivx` binder with your front matter and chapters as documents, named parts
  as folders, and each chapter's prose as RTF with emphasis carried through. It
  opens directly in Scrivener — no import step, no rebuild.

- **Vellum (.docx)** — a `.docx` tuned for [Vellum](https://vellum.pub) import.
  Chapter and part titles are styled **Heading 1**, which is where Vellum splits
  chapters, so your chapters arrive as separate, correctly-ordered elements with
  emphasis intact. There is no manuscript title page — Vellum applies its own
  design. In Vellum, use *File → Import Word File…* and pick the `.vellum.docx`.

- **EPUB (.epub)** — a finished ebook, the standard distribution format for Apple
  Books, Kobo, and Kindle. It carries a navigable table of contents built from
  your part/chapter structure, book metadata (title, subtitle, author, series),
  a cover and other embedded images, epigraphs set apart, and `*`/`**` emphasis
  as italic and bold. It is EPUB 3 with an EPUB 2 NCX fallback for older readers,
  full structural semantics and accessibility metadata, and it is structured to
  validate against [epubcheck](https://www.w3.org/publishing/epubcheck/).

- **PDF** — two variants. *PDF (screen)* is a paginated, fixed-layout proof for
  beta readers and submissions. *PDF (print interior)* is the book block for
  print-on-demand: a chosen trim size, mirrored inside/outside margins with a
  binding gutter, an optional bleed, embedded and subset fonts, drop caps,
  ornaments, running heads, and folios. Details on the print interior are in
  [The print interior](#the-print-interior) below.

There is no native "Vellum format" export — Vellum is an import target, and the
Vellum `.docx` exists to feed it. Likewise there is no separate KDP or
IngramSpark *format*; those stores take the standard print PDF and EPUB, prepared
with the right trim and bleed (see [Prepare for KDP / IngramSpark](#prepare-for-kdp--ingramspark)).


## Short stories

A short story project (see the user guide) exports as a story rather than a
book. The **Word** export is in standard short story manuscript format, ready to
submit: your contact details single-spaced at the top left of page one (from the
`[author]` section of your config, or just your byline when that's unset), the
word count rounded to the nearest hundred at the top right ("about 3,500 words";
the nearest ten under 1,000), the title and byline about a third of the way down,
and the story starting right beneath them. There's no title page, no table of
contents, no chapter heading, and no page break before the prose. Scenes are
separated by a centered `#`, a centered END closes the story, and the running
header reads `Surname / Short title / page`. A dedication or afterword follows the
story on its own page.

The other formats keep their usual look but treat the story as one piece: the
Markdown, EPUB, and PDF exports don't repeat the title as a chapter heading, and
the PDF has no one-line Contents page.

## The design system

Two per-project tables shape the designed exports — the EPUB and the print PDF.
Both live in `book.toml` (they describe one book, so they have no global
equivalent). Neither affects the manuscript `.docx`, the Markdown, or the
Scrivener bundle, which stay deliberately plain.

Set most of this from the **Set export style…** entry in the export menu; the
raw keys are here for reference and for the values the menu doesn't cover.

### `[style]` — the ebook and print typography

The quickest way in is a **preset** — a named look that bundles the scene break,
drop cap, and ornaments:

```toml
[style]
preset = "classic"
```

The presets are `classic`, `modern`, `literary`, `romance`, `thriller`,
`fantasy`, and `minimalist`. Any key you set below a preset overrides that part
of it.

**Scene breaks and ornaments** are glyphs by default:

```toml
[style]
scene_break    = "* * *"   # the divider between scenes; default "#"
title_ornament = "❦"       # centered on the title page
part_ornament  = "❦"       # centered on each part-divider page
```

Any of these can be an image instead — `scene_break_image`, `title_image`,
`chapter_heading_image` (set above each chapter title), and `cover_image` all
take a project-relative path. Each image has an optional `*_alt` key for the
accessibility description; without one, an image is treated as decorative
(`cover_image_alt` falls back to the book title). Images may be PNG, JPEG, GIF,
or WebP; SVG is honored in the EPUB but reported and skipped in the PDF, which
takes raster only.

**Drop caps** turn the first paragraph of each chapter into a three-line dropped
initial:

```toml
[style]
drop_cap = true
```

**Fonts.** By default the print PDF embeds Liberation Serif (the body, four
weights, SIL OFL), DejaVu Sans as a fallback, and DejaVu Sans Mono for fenced
code blocks. It renders the full Unicode those faces cover — Latin, Greek,
Cyrillic, smart punctuation, and decorative ornaments all show — using Type0
composite fonts over an Identity-H CMap, and it subsets each face to the glyphs
you actually use so files stay small. Chinese and other CJK scripts need an
author-supplied CJK font.

To override the defaults, point `body_font` and `heading_font` at font files:

```toml
[style]
body_font    = "fonts/CrimsonPro.ttf"
heading_font = "fonts/Cinzel.ttf"
heading_align = "center"    # left / center / right / justify
```

Paths are relative to the project. A sibling `<stem>-italic.<ext>` is picked up
automatically for the italic face. Supported types are `.ttf`, `.otf`, `.woff`,
and `.woff2`. The **Set export style…** menu can also browse a font library and,
when you pick a family, download it into the project's `fonts/` directory as
`fonts/<family>.ttf` with its license saved beside it as
`fonts/<family>.LICENSE.txt`, so the license travels with the project.

**Front-matter toggles**:

```toml
[style]
half_title  = true    # a bare half-title page before the full title page; off by default
series_list = true    # an "Also in this series" page; on by default for series books
```

### `[print]` — trim, margins, and the spine

The print interior is built from a `[print]` table:

```toml
[print]
trim    = "6x9"     # also "5x8" and "5.5x8.5"; default "6x9"
bleed   = 0.125     # inches past the trim edge for full-bleed art; default 0 (none)
caliper = 0.0025    # per-page paper thickness in inches; feeds the spine width
```

Three trim sizes are supported — `6x9`, `5x8`, and `5.5x8.5` — and that is the
one current limitation. Margins are mirrored by page parity: the default 6×9
interior uses 0.875″ inside (a 0.5″ outer margin plus a 0.375″ gutter for the
binding), 0.5″ outside, and 0.75″ top and bottom. `bleed` runs art past the trim
edge for covers and full-bleed pages; leave it unset for a clean trim, or set the
0.125″ that print-on-demand stores want. `caliper` is the per-page paper
thickness — the wrap cover's spine width is `interior page count × caliper` — and
defaults to a sane value when unset; set it to match your printer's stock.


## Preflight

Before you upload, run the export **preflight** check from the export menu. It
inspects the book against the requirements of a print/ebook release and reports
each finding as **PASS**, **WARN**, or **FAIL**. A failure is a hard problem a
store will reject; a warning is worth a look but won't block you.

Preflight runs these checks:

- **ISBN** — presence and a valid check digit. It validates both ISBN-10
  (mod-11, with a trailing `X` allowed) and ISBN-13 (mod-10), ignoring hyphens
  and spaces. A wrong check digit fails.
- **Copyright page** — warns when the publishing metadata is too thin to
  generate a copyright page.
- **Language tag** — checks the BCP-47 structure; a malformed tag fails, and an
  unset one warns (the export defaults to `en`).
- **Cover** — fails when no cover image is set, and checks a present cover's
  resolution against the PDF writer's DPI floor.
- **Bleed / cover alignment** (print only) — surfaces any trim warning, and fails
  if bleed is set but no cover is present to fill it.
- **Empty chapters** — warns and names any body chapter with zero words.
- **Unbalanced emphasis** (optional) — names chapters with an odd number of `*`
  or `_` markers, which usually means an emphasis run was left open.

A note on font licensing and epubcheck: preflight does not run epubcheck itself,
and it does not audit font licenses. The EPUB writer is *structured* to pass
epubcheck, and the font picker saves each downloaded family's license beside it —
but the preflight report covers the metadata and structural checks above, not
external validation.


## Prepare for KDP / IngramSpark

The **Prepare for KDP** and **Prepare for IngramSpark** entries in the export
menu produce a print-on-demand bundle in one step. Each writes two files under
`export/`:

- the print interior PDF (`<book>.print.pdf`), and
- a reflowable EPUB (`<book>.epub`).

Before writing, "prepare" applies the store defaults and saves them into
`book.toml` so later exports stay aligned: a **6×9 trim**, a **0.125″ bleed**
(the allowance those stores expect), and an `en` language tag if none is set. It
also runs preflight first — a FAIL blocks the bundle, warnings are reported but
don't stop it.

KDP and IngramSpark take the same 6×9 / 0.125″ bundle today; the two entries are
kept distinct for the label and for any future per-store defaults.

For the cover, the **PDF wrap cover** export (offered for standalone books)
writes the press-ready single-sheet wrap — back cover, spine, and front cover
flattened onto one page at `2 × trim + spine + 2 × bleed`, a PDF/X-3 grayscale
file with the ISBN/barcode safe zone marked on the back and fold guides on the
spine. The spine width is sized from *this book's* interior page count at the
same trim and bleed as the print interior, so the cover matches the block that
ships beside it.


## Samples and review copies

You don't have to export the whole book. Two menu groups produce partial or
purpose-built exports.

**Samples** export a span of the active book in any format — a single chapter, a
single part, or a range (offered as "first three chapters" when the book has at
least three). The output filename carries a scope tag so a sample never
overwrites the full export. Samples are the same compiled output as a whole-book
export, just narrowed to the span.

**Review copy** export writes a `.docx` built to be marked up: every body line is
numbered down the margin, the page carries generous 2″ margins for handwritten or
tracked notes, and the inline annotations you left while writing — which every
other export strips — ride along as Word comments anchored to the phrases they
were left on. Send it to a beta reader or editor, get it back marked up, and
Quill can import the annotations again.


## Previewing before a full export

Two entries in the export menu let you see a look without exporting the whole
book. Both render a small fixed sample — a title page, a chapter opening with a
drop cap, a scene break, a part divider — through the *same* writers a real
export uses, so what you see is what you'll get. Both write only a temp file and
never touch your manuscript.

- **Preview style…** renders the sample as an EPUB and opens it in your system
  ebook reader, so you can check fonts, images, drop caps, and ornaments the way
  the `[style]` table will render them.

- **Preview print interior…** renders the sample as a print PDF at your current
  `[print]` trim, margins, and bleed and opens it in your PDF viewer, so you can
  check the page geometry and typography before committing to a full print
  export.


## See also

- [`configuration.md`](configuration.md) — global settings in the home config file.
- [`guide.md`](guide.md) — the in-app F1 guide, including the export walkthrough.
- [`version-control.md`](version-control.md) — drafts, milestones, and history.
