#!/usr/bin/env bash
# Run the UI tests on the newest base iPhone, e, Pro, Pro Max, and Air
# simulator this Xcode has (#1289), building once. PhoneLayoutUITests checks
# each one upright and sideways, and none of its checks may skip on these
# phones. CI runs this; it needs a built core (ios/build-core.sh) and XcodeGen.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# "iPhone 17", "iPhone 17e", "iPhone 18 Pro", "iPhone 18 Pro Max": the newest
# of a kind has the highest number. The Air has no number yet ("iPhone Air"),
# so a numbered one ("iPhone Air 2") sorts after it. The Duo isn't picked.
names="$(xcrun simctl list devices available | sed -nE 's/^ +(iPhone [0-9A-Za-z ]+[0-9A-Za-z]) \(.*/\1/p' | sort -u)"
newest() { echo "$names" | grep -E "^$1\$" | sort -V | tail -1; }
phones=(
  "$(newest 'iPhone [0-9]+')"
  "$(newest 'iPhone [0-9]+e')"
  "$(newest 'iPhone [0-9]+ Pro')"
  "$(newest 'iPhone [0-9]+ Pro Max')"
  "$(newest 'iPhone Air( [0-9]+)?')"
)
for phone in "${phones[@]}"; do
  [ -n "$phone" ] || {
    echo "this Xcode is missing a base iPhone, e, Pro, Pro Max, or Air simulator" >&2
    exit 1
  }
done

cd "$ROOT/ios/App"
xcodegen generate --quiet
build=(-project Quill.xcodeproj -scheme Quill -derivedDataPath "$ROOT/ios/build/DerivedData" CODE_SIGNING_ALLOWED=NO)
xcodebuild "${build[@]}" -destination 'generic/platform=iOS Simulator' -quiet build-for-testing

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
# Every phone runs even after one fails, so one run shows them all.
failed=()
for phone in "${phones[@]}"; do
  echo "== $phone"
  xcodebuild "${build[@]}" -destination "platform=iOS Simulator,name=$phone" test-without-building 2>&1 |
    tee "$log" | grep -E "^Test Case .*(passed|failed|skipped)|: error: |TEST EXECUTE (SUCCEEDED|FAILED)" || true
  if ! grep -q "TEST EXECUTE SUCCEEDED" "$log"; then
    failed+=("$phone: tests failed")
  elif grep -qE "^Test Case .*PhoneLayoutUITests.* skipped" "$log"; then
    failed+=("$phone: a layout check skipped instead of running")
  fi
done
if [ "${#failed[@]}" -gt 0 ]; then
  printf '%s\n' "${failed[@]}" >&2
  exit 1
fi
