#!/usr/bin/env bash
# Run QuillKit's Swift tests (#1230) against the macOS slice of the core.
# Builds that slice first unless --no-build is passed. With full Xcode this is
# plain `swift test`; with only the Command Line Tools, Swift Testing lives
# outside the default search paths, so point the compiler and loader at it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ "${1:-}" != "--no-build" ]; then
  "$ROOT/ios/build-core.sh" --host
fi
cd "$ROOT/ios/QuillKit"
flags=()
dev="$(xcode-select -p)"
if [[ "$dev" == *CommandLineTools* ]]; then
  fw="$dev/Library/Developer/Frameworks"
  lib="$dev/Library/Developer/usr/lib"
  flags=(-Xswiftc -F -Xswiftc "$fw" -Xlinker -F -Xlinker "$fw"
    -Xlinker -rpath -Xlinker "$fw" -Xlinker -rpath -Xlinker "$lib")
fi
swift test "${flags[@]}"
