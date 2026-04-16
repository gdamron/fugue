#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?version required}"
OUT_DIR="${2:?output directory required}"
WASM_DIR="${3:?wasm bindgen output directory required}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE_DIR="$OUT_DIR/fugue-$VERSION-web"

mkdir -p "$STAGE_DIR"

cp -R "$WASM_DIR"/. "$STAGE_DIR/"
cp "$ROOT/LICENSE" "$STAGE_DIR/"

cat > "$STAGE_DIR/README.md" <<EOF
# Fugue $VERSION for Web

Contents:

- wasm-bindgen generated JS glue
- \`.wasm\` binary
- \`LICENSE\`

Use the exported \`FugueEngine\` class to load invention JSON, inspect and mutate
the graph (\`listModules\`, \`addModule\`, \`connect\`, etc.), set controls, and
render interleaved stereo sample blocks inside a Web Audio host. \`code\` modules
are orchestration-only on web and are expected to run in the surrounding JS host.
EOF

tar -C "$OUT_DIR" -czf "$OUT_DIR/fugue-$VERSION-web.tar.gz" "$(basename "$STAGE_DIR")"
rm -rf "$STAGE_DIR"
