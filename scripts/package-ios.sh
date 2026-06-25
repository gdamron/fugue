#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?version required}"
OUT_DIR="${2:?output directory required}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_DIR="$OUT_DIR/fugue-$VERSION-ios"
XCFRAMEWORK_DIR="$STAGE_DIR/Fugue.xcframework"

mkdir -p "$STAGE_DIR"

# `-create-xcframework` rejects two libraries for the same platform+arch family,
# so the arm64 and x86_64 *simulator* slices must be combined into a single fat
# library with `lipo` before being handed to xcodebuild as one simulator entry.
SIM_LIB="$OUT_DIR/libfugue-ios-sim.a"
lipo -create \
  "$ROOT/target/aarch64-apple-ios-sim/release/libfugue.a" \
  "$ROOT/target/x86_64-apple-ios/release/libfugue.a" \
  -output "$SIM_LIB"

xcodebuild -create-xcframework \
  -library "$ROOT/target/aarch64-apple-ios/release/libfugue.a" -headers "$ROOT/include" \
  -library "$SIM_LIB" -headers "$ROOT/include" \
  -output "$XCFRAMEWORK_DIR"

rm -f "$SIM_LIB"

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
