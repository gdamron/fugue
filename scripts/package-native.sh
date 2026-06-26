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
    STATIC_LIB="libfugue.a"
    SHARED_LIB="libfugue.dylib"
    IMPORT_LIB=""
    ;;
  *linux*)
    STATIC_LIB="libfugue.a"
    SHARED_LIB="libfugue.so"
    IMPORT_LIB=""
    ;;
  *windows-msvc)
    STATIC_LIB="fugue.lib"
    SHARED_LIB="fugue.dll"
    IMPORT_LIB="fugue.dll.lib"
    ;;
  *)
    echo "unsupported native target: $TARGET" >&2
    exit 1
    ;;
esac

cp "$BUILD_DIR/$STATIC_LIB" "$STAGE_DIR/lib/"
cp "$BUILD_DIR/$SHARED_LIB" "$STAGE_DIR/lib/"
if [[ -n "$IMPORT_LIB" && -f "$BUILD_DIR/$IMPORT_LIB" ]]; then
  cp "$BUILD_DIR/$IMPORT_LIB" "$STAGE_DIR/lib/"
fi
IMPORT_LIB_CONTENT=""
if [[ -n "$IMPORT_LIB" ]]; then
  IMPORT_LIB_CONTENT="- \`lib/$IMPORT_LIB\`"
fi
cp "$ROOT/include/fugue.h" "$STAGE_DIR/"
cp "$ROOT/LICENSE" "$STAGE_DIR/"

cat > "$STAGE_DIR/README.md" <<EOF
# Fugue $VERSION for $TARGET

Contents:

- \`lib/$STATIC_LIB\`
- \`lib/$SHARED_LIB\`
$IMPORT_LIB_CONTENT
- \`fugue.h\`
- \`LICENSE\`

This package exposes the host-driven render API declared in \`fugue.h\`.
Render interleaved stereo \`f32\` frames from caller-owned buffers and set
runtime controls without embedding Rust in the host application.
EOF

tar -C "$OUT_DIR" -czf "$OUT_DIR/fugue-$VERSION-$TARGET.tar.gz" "$(basename "$STAGE_DIR")"
rm -rf "$STAGE_DIR"
