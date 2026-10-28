#!/usr/bin/env bash
# Typecheck the iPhone app's Swift without Xcode (#1230). The macOS SDK in the
# Command Line Tools carries UIKit for Mac Catalyst, so the app, QuillKit, and
# the generated bindings typecheck together as one Catalyst module. That
# catches Swift errors locally; CI still builds the real iOS app.
#
#   ios/typecheck-app.sh            build the Mac slice first, then typecheck
#   ios/typecheck-app.sh --no-build reuse the last ios/build-core.sh output
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ "${1:-}" != "--no-build" ]; then
  "$ROOT/ios/build-core.sh" --host
fi
SDK="$(xcrun --sdk macosx --show-sdk-path)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src" "$work/include"
cp "$ROOT"/ios/QuillKit/Sources/QuillCore/QuillCore.swift \
  "$ROOT"/ios/QuillKit/Sources/QuillKit/*.swift \
  "$ROOT"/ios/App/Quill/*.swift "$work/src/"
cp "$ROOT"/ios/build/QuillFFI.xcframework/macos-arm64/Headers/* "$work/include/"
# One module instead of three: drop the cross-module imports, and name the
# module QuillCore so `QuillCore.configure` still resolves.
sed -i '' -e '/^import QuillKit$/d' -e '/^@_exported import QuillCore$/d' "$work"/src/*.swift
swiftc -typecheck -module-name QuillCore -swift-version 5 \
  -target arm64-apple-ios18.0-macabi -sdk "$SDK" \
  -Fsystem "$SDK/System/iOSSupport/System/Library/Frameworks" \
  -I "$SDK/System/iOSSupport/usr/include" -I "$work/include" "$work"/src/*.swift
echo "app typechecks"
