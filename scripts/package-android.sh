#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?version required}"
OUT_DIR="${2:?output directory required}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${FUGUE_TARGET_DIR:-$ROOT/target}"
STAGE_DIR="$OUT_DIR/fugue-$VERSION-android"

mkdir -p "$STAGE_DIR/lib/arm64-v8a" "$STAGE_DIR/lib/x86_64"

cp "$TARGET_DIR/aarch64-linux-android/release/libfugue.so" "$STAGE_DIR/lib/arm64-v8a/"
cp "$TARGET_DIR/x86_64-linux-android/release/libfugue.so" "$STAGE_DIR/lib/x86_64/"
cp "$ROOT/include/fugue.h" "$STAGE_DIR/"
cp "$ROOT/LICENSE" "$STAGE_DIR/"
bash "$ROOT/scripts/generate-licenses.sh" "$STAGE_DIR/THIRD_PARTY_LICENSES.txt" \
  aarch64-linux-android x86_64-linux-android

cat > "$STAGE_DIR/README.md" <<EOF
# Fugue $VERSION for Android

Contents:

- \`lib/arm64-v8a/libfugue.so\`
- \`lib/x86_64/libfugue.so\`
- \`fugue.h\`
- \`LICENSE\`
- \`THIRD_PARTY_LICENSES.txt\`

These shared libraries expose the C render API and are intended for JNI or
other Android host integration layers.
EOF

tar -C "$OUT_DIR" -czf "$OUT_DIR/fugue-$VERSION-android.tar.gz" "$(basename "$STAGE_DIR")"
rm -rf "$STAGE_DIR"
