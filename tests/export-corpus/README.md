# Export corpus

Small sample books that between them use every feature Quill's exporters
handle, so validators can check the output (#1301 to #1303).
`scripts/check-exports.sh` exports each to EPUB, Word, PDF, a print interior,
and a wrap cover with the core's own export code, then runs EPUBCheck and Ace
over the EPUBs, the Open XML SDK and LibreOffice over the Word files, and a
PDF/X-4 preflight and qpdf over the print PDFs. CI runs it on every pull
request.

- `novel/`: parts, front and back matter (a dedication, acknowledgments), a
  book epigraph and a chapter epigraph, scene breaks, emphasis, a cover image
  with alt text, publishing metadata, text in French, Greek, and Japanese, and a
  5.5 by 8.5 print trim with a 0.125" bleed.
- `series/`: a two-book series (`[[books]]`), exported as one omnibus and as
  its open book.
- `story/` and `story-two/`: short stories with scenes.
- `collection/`: a collection linking the two stories by relative path, with a
  credit line on one.

Fix the exporter, not the samples, when a check fails. Add a sample when the
exporters learn a feature (footnotes have no Quill syntax yet).
