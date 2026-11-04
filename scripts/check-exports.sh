#!/usr/bin/env bash
# Export the sample books in tests/export-corpus with the core's own export
# code and validate the files (#1301): every EPUB must pass EPUBCheck with no
# errors or warnings, and the Ace accessibility checker with no violations.
# CI runs this on every pull request.
#
#   scripts/check-exports.sh            export, EPUBCheck, then Ace
#   SKIP_ACE=1 scripts/check-exports.sh  without Ace (it needs Node and Chrome)
#
# EPUBCHECK is the command that runs EPUBCheck (default: `epubcheck`, as
# Homebrew installs it; CI uses `java -jar epubcheck.jar`). ACE likewise
# (default: `ace`, from `npm install -g @daisy/ace`).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EPUBCHECK="${EPUBCHECK:-epubcheck}"
ACE="${ACE:-ace}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# Opening a book writes its lock under .quill/, so export from a copy.
cp -R "$ROOT/tests/export-corpus" "$work/books"
cargo run -q -p quill-core --example export_corpus -- "$work/books" "$work/out"

failed=()
for epub in "$work"/out/*.epub; do
  name="$(basename "$epub")"
  echo "== EPUBCheck: $name"
  if ! $EPUBCHECK --failonwarnings "$epub"; then
    failed+=("$name: EPUBCheck")
  fi
done

if [ -z "${SKIP_ACE:-}" ]; then
  for epub in "$work"/out/*.epub; do
    name="$(basename "$epub")"
    echo "== Ace: $name"
    # Exit 2 on any accessibility violation, not only on a crash.
    if ! $ACE --silent --return-2-on-validation-error --outdir "$work/ace/${name%.epub}" "$epub"; then
      failed+=("$name: Ace")
      # The report says which rule failed, and where.
      [ -f "$work/ace/${name%.epub}/report.json" ] &&
        node -e '
          const r = require(process.argv[1]);
          for (const a of r.assertions || []) for (const t of a.assertions || [])
            if (t["earl:result"]["earl:outcome"] !== "pass")
              console.log(`  ${t["earl:test"]["dct:title"]}: ${t["earl:result"]["dct:description"] || ""}`);
        ' "$work/ace/${name%.epub}/report.json" || true
    fi
  done
fi

if [ "${#failed[@]}" -gt 0 ]; then
  printf 'failed: %s\n' "${failed[@]}" >&2
  exit 1
fi
echo "every export passed"
