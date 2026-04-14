#!/usr/bin/env bash

set -euo pipefail

TARGET="${1:?target triple required}"
VERSION="${2:?version required}"
OUT_DIR="${3:?output directory required}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$ROOT/target/$TARGET/release"
STAGE_DIR="$OUT_DIR/fugue-$VERSION-$TARGET"

mkdir -p "$STAGE_DIR/lib"

case "$TARGET" in
  *apple-darwin)
    SHARED_EXT="dylib"
    ;;
  *linux*)
    SHARED_EXT="so"
    ;;
  *)
    echo "unsupported native target: $TARGET" >&2
    exit 1
    ;;
esac

cp "$BUILD_DIR/libfugue.a" "$STAGE_DIR/lib/"
cp "$BUILD_DIR/libfugue.$SHARED_EXT" "$STAGE_DIR/lib/"
cp "$ROOT/include/fugue.h" "$STAGE_DIR/"
cp "$ROOT/LICENSE" "$STAGE_DIR/"

cat > "$STAGE_DIR/README.md" <<EOF
# Fugue $VERSION for $TARGET

Contents:

- \`lib/libfugue.a\`
- \`lib/libfugue.$SHARED_EXT\`
- \`fugue.h\`
- \`LICENSE\`

This package exposes the host-driven render API declared in \`fugue.h\`.
Render interleaved stereo \`f32\` frames from caller-owned buffers and set
runtime controls without embedding Rust in the host application.
EOF

tar -C "$OUT_DIR" -czf "$OUT_DIR/fugue-$VERSION-$TARGET.tar.gz" "$(basename "$STAGE_DIR")"
rm -rf "$STAGE_DIR"
