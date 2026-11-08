#!/usr/bin/env bash
# Acceptance check C4: a fyug-product build pinned to core's change produces
# none of core's C libraries (libfugue.a, libfugue.dylib, libfugue.so,
# fugue.dll and its import libraries) in its target directory.
#
# Usage: check-product.sh <fyug-product checkout> [target dir]
#
# The checkout must already pin core to the change (Cargo.toml's fugue rev).
# The target dir defaults to CARGO_TARGET_DIR, then <checkout>/target. Stale C
# libraries left by an earlier build are removed first, so only this build
# counts. PRODUCT_BUILD_ARGS overrides the build arguments (default:
# "build --release --workspace").
#
# Prints PASS or FAIL and exits non-zero on failure.

set -euo pipefail

PRODUCT="$(cd "${1:?usage: check-product.sh <fyug-product checkout> [target dir]}" && pwd)"
TD="${2:-${CARGO_TARGET_DIR:-$PRODUCT/target}}"
read -r -a BUILD_ARGS <<<"${PRODUCT_BUILD_ARGS:-build --release --workspace}"
# Uplifted names, plus the hashed copies cargo keeps under deps/ for a
# dependency (libfugue-<hash>.a and so on).
NAMES=(libfugue.a libfugue.dylib libfugue.so fugue.dll fugue.lib fugue.dll.lib
  'libfugue-*.a' 'libfugue-*.dylib' 'libfugue-*.so' 'fugue-*.dll' 'fugue-*.lib' 'fugue-*.dll.lib')

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failed=0

find_c_libs() {
  local expr=() name
  for name in "${NAMES[@]}"; do expr+=(-name "$name" -o); done
  unset 'expr[${#expr[@]}-1]'
  [[ -d "$TD" ]] || return 0
  find "$TD" \( "${expr[@]}" \) -print
}

echo "fyug-product at $PRODUCT pins: $(grep -E '^fugue *=' "$PRODUCT/Cargo.toml" || echo 'no fugue line in Cargo.toml')"
find_c_libs | while IFS= read -r stale; do rm -f "$stale"; done

if ! (cd "$PRODUCT" && CARGO_TARGET_DIR="$TD" cargo "${BUILD_ARGS[@]}" --message-format=json-render-diagnostics) >"$work/build.json"; then
  echo "FAIL: C4 cargo ${BUILD_ARGS[*]} builds fyug-product"
  exit 1
fi

reported="$(python3 - "$work/build.json" <<'PY'
import json, os, re, sys
pattern = re.compile(r"^(libfugue(-[0-9a-f]+)?\.(a|dylib|so)|fugue(-[0-9a-f]+)?\.(dll|lib|dll\.lib))$")
for line in open(sys.argv[1]):
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    if msg.get("reason") == "compiler-artifact":
        for f in msg.get("filenames", []):
            if pattern.match(os.path.basename(f)):
                print(f)
PY
)"
if [[ -n "$reported" ]]; then
  echo "FAIL: C4 the product build reports no core C library artifacts (got: $(tr '\n' ' ' <<<"$reported"))"
  failed=1
else
  echo "PASS: C4 the product build reports no core C library artifacts"
fi

found="$(find_c_libs)"
if [[ -n "$found" ]]; then
  echo "FAIL: C4 $TD holds none of core's C libraries (found: $(tr '\n' ' ' <<<"$found"))"
  failed=1
else
  echo "PASS: C4 $TD holds none of core's C libraries"
fi

exit "$failed"
