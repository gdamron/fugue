#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?version required}"
OUT_DIR="${2:?output directory required}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_DIR="$OUT_DIR/fugue-$VERSION-ios"
XCFRAMEWORK_DIR="$STAGE_DIR/Fugue.xcframework"

mkdir -p "$STAGE_DIR"

xcodebuild -create-xcframework \
  -library "$ROOT/target/aarch64-apple-ios/release/libfugue.a" -headers "$ROOT/include" \
  -library "$ROOT/target/aarch64-apple-ios-sim/release/libfugue.a" -headers "$ROOT/include" \
  -library "$ROOT/target/x86_64-apple-ios/release/libfugue.a" -headers "$ROOT/include" \
  -output "$XCFRAMEWORK_DIR"

cp "$ROOT/LICENSE" "$STAGE_DIR/"

cat > "$STAGE_DIR/README.md" <<EOF
# Fugue $VERSION for iOS

Contents:

- \`Fugue.xcframework\`
- \`LICENSE\`

The xcframework contains the static Fugue render engine for device and simulator
architectures, plus the public C header needed to call the API.
EOF

tar -C "$OUT_DIR" -czf "$OUT_DIR/fugue-$VERSION-ios-xcframework.tar.gz" "$(basename "$STAGE_DIR")"
rm -rf "$STAGE_DIR"
