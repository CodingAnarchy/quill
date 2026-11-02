# Export corpus

Small sample books that between them use every feature Quill's exporters
handle, so a validator can check the output (#1301). `scripts/check-exports.sh`
exports each to EPUB (and DOCX and PDF) with the core's own export code and
runs EPUBCheck and the Ace accessibility checker over the EPUBs; CI runs it on
every pull request.

- `novel/`: parts, front and back matter (a dedication, acknowledgments), a
  book epigraph and a chapter epigraph, scene breaks, emphasis, a cover image
  with alt text, publishing metadata, and text in French, Greek, and Japanese.
- `series/`: a two-book series (`[[books]]`), exported as one omnibus and as
  its open book.
- `story/` and `story-two/`: short stories with scenes.
- `collection/`: a collection linking the two stories by relative path, with a
  credit line on one.

Fix the exporter, not the samples, when a check fails. Add a sample when the
exporters learn a feature (footnotes have no Quill syntax yet).
