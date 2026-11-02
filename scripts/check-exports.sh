#!/usr/bin/env bash
# Export the sample books in tests/export-corpus with the core's own export
# code and validate the files. CI runs this on every pull request.
#
# - EPUB (#1301): EPUBCheck with no errors or warnings, and the Ace
#   accessibility checker with no violations.
# - Word (#1302): the Office Open XML schemas, through Microsoft's Open XML SDK
#   (tools/docx-validate, needs .NET 8), and a LibreOffice headless conversion
#   to PDF as a smoke test that another program reads the file.
#
#   scripts/check-exports.sh        every check whose tool is installed
#   REQUIRE_ALL=1 scripts/...       fail when a tool is missing (CI does)
#   SKIP_ACE=1 scripts/...          without Ace (it needs Node and Chrome)
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

# A check whose tool isn't installed is skipped with a note, or fails under
# REQUIRE_ALL, so CI can't pass a check it never ran.
have() {
  if command -v "$1" >/dev/null 2>&1; then return 0; fi
  if [ -n "${REQUIRE_ALL:-}" ]; then
    failed+=("$2: $1 isn't installed")
  else
    echo "== skipping $2: $1 isn't installed"
  fi
  return 1
}

if [ -z "${SKIP_ACE:-}" ] && have "${ACE%% *}" Ace; then
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

if have dotnet "the Open XML check"; then
  if ! dotnet run --project "$ROOT/tools/docx-validate" -c Release -- "$work"/out/*.docx; then
    failed+=("DOCX: Open XML schema")
  fi
fi

if have soffice "the LibreOffice check"; then
  mkdir -p "$work/lo"
  for docx in "$work"/out/*.docx; do
    name="$(basename "$docx" .docx)"
    echo "== LibreOffice: $name.docx"
    soffice --headless --convert-to pdf --outdir "$work/lo" "$docx" >/dev/null 2>&1 || true
    if [ ! -s "$work/lo/$name.pdf" ]; then
      failed+=("$name.docx: LibreOffice couldn't open it")
    fi
  done
fi

if [ "${#failed[@]}" -gt 0 ]; then
  printf 'failed: %s\n' "${failed[@]}" >&2
  exit 1
fi
echo "every export passed"
