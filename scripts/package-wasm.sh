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
cp "$ROOT/web/fugue-code-host.js" "$STAGE_DIR/"

cat > "$STAGE_DIR/README.md" <<EOF
# Fugue $VERSION for Web

Contents:

- wasm-bindgen generated JS glue
- `fugue-code-host.js` helper for running `code` modules in host JavaScript
- \`.wasm\` binary
- \`LICENSE\`

Use the exported \`FugueEngine\` class to load invention JSON, inspect and mutate
the graph (\`listModules\`, \`addModule\`, \`connect\`, etc.), set controls, and
render interleaved stereo sample blocks inside a Web Audio host. \`code\` modules
are orchestration-only on web and should be run with the packaged
\`WasmCodeHost\` helper from \`fugue-code-host.js\`. Preferred script style is
plain top-level lifecycle functions like \`function init() {}\`.
EOF

tar -C "$OUT_DIR" -czf "$OUT_DIR/fugue-$VERSION-web.tar.gz" "$(basename "$STAGE_DIR")"
rm -rf "$STAGE_DIR"
