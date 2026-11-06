#!/usr/bin/env bash
# Dry run of the release's license notices, runnable outside a release.
#
# Builds the notices for every library archive's targets with the pinned
# cargo-about, packages the archives whose layout can be exercised without a
# compiler toolchain for their platform (native, Android and web, from stub
# libraries), and verifies each one carries THIRD_PARTY_LICENSES.txt at its top
# level next to LICENSE and the libraries. It also proves that packaging fails,
# and leaves no archive behind, when a dependency's license is not accepted.
# The iOS archive needs xcodebuild, so only its notices are generated here.
#
# Needs cargo and cargo-about (see scripts/generate-licenses.sh). Takes a few
# minutes; CI runs it on changes to the packaging, the dependency tree or the
# release workflow.
#
# Usage: scripts/check-release-notices.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failed=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failed=1; }

# expect_entries <archive> <top-dir> <entry>...: the archive holds each entry.
expect_entries() {
  local archive="$1" top="$2" listing entry
  shift 2
  [[ -f "$archive" ]] || { fail "$(basename "$archive") was not produced"; return; }
  listing="$(tar -tzf "$archive" | sed 's|^\./||')"
  for entry in "$@"; do
    if grep -qx "$top/$entry" <<<"$listing"; then
      pass "$(basename "$archive") holds $entry"
    else
      fail "$(basename "$archive") lacks $top/$entry"
    fi
  done
}

# The notices must be real: not an empty or placeholder file.
expect_notices_content() {
  local archive="$1" top="$2" dir notices
  dir="$work/extract-$(basename "$archive")"
  mkdir -p "$dir" && tar -xzf "$archive" -C "$dir"
  notices="$dir/$top/THIRD_PARTY_LICENSES.txt"
  if [[ -s "$notices" ]] && grep -Eq '^  fugue [0-9]' "$notices" && grep -qF 'Permission is hereby granted, free of charge' "$notices" \
    && grep -qF 'Version 2.0, January 2004' "$notices"; then
    pass "$(basename "$archive") notices list the fugue crate and carry the MIT and Apache texts"
  else
    fail "$(basename "$archive") notices are incomplete"
  fi
}

stub() { mkdir -p "$(dirname "$1")" && echo stub >"$1"; }

export FUGUE_TARGET_DIR="$work/target"
out="$work/dist"
mkdir -p "$out"

echo "--- native archives"
for target in x86_64-unknown-linux-gnu aarch64-apple-darwin x86_64-pc-windows-msvc; do
  case "$target" in
    *apple-darwin) libs=(libfugue.a libfugue.dylib) ;;
    *linux*) libs=(libfugue.a libfugue.so) ;;
    *windows*) libs=(fugue.lib fugue.dll) ;;
  esac
  for lib in "${libs[@]}"; do stub "$FUGUE_TARGET_DIR/$target/release/$lib"; done
  bash "$ROOT/scripts/package-native.sh" "$target" "$VERSION" "$out" >/dev/null
  top="fugue-$VERSION-$target"
  expect_entries "$out/$top.tar.gz" "$top" LICENSE THIRD_PARTY_LICENSES.txt fugue.h "lib/${libs[0]}" "lib/${libs[1]}"
  expect_notices_content "$out/$top.tar.gz" "$top"
done

echo "--- android archive"
stub "$FUGUE_TARGET_DIR/aarch64-linux-android/release/libfugue.so"
stub "$FUGUE_TARGET_DIR/x86_64-linux-android/release/libfugue.so"
bash "$ROOT/scripts/package-android.sh" "$VERSION" "$out" >/dev/null
expect_entries "$out/fugue-$VERSION-android.tar.gz" "fugue-$VERSION-android" LICENSE THIRD_PARTY_LICENSES.txt fugue.h
expect_notices_content "$out/fugue-$VERSION-android.tar.gz" "fugue-$VERSION-android"

echo "--- web archive"
stub "$work/wasm/fugue.js"
stub "$work/wasm/fugue_bg.wasm"
bash "$ROOT/scripts/package-wasm.sh" "$VERSION" "$out" "$work/wasm" >/dev/null
expect_entries "$out/fugue-$VERSION-web.tar.gz" "fugue-$VERSION-web" LICENSE THIRD_PARTY_LICENSES.txt fugue.js
expect_notices_content "$out/fugue-$VERSION-web.tar.gz" "fugue-$VERSION-web"

echo "--- iOS notices (the archive itself needs xcodebuild)"
if bash "$ROOT/scripts/generate-licenses.sh" "$work/ios-notices.txt" \
  aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios >/dev/null 2>&1 && [[ -s "$work/ios-notices.txt" ]]; then
  pass "notices generate for the iOS targets"
else
  fail "notices do not generate for the iOS targets"
fi
grep -q 'generate-licenses.sh' "$ROOT/scripts/package-ios.sh" \
  && pass "package-ios.sh generates the notices" || fail "package-ios.sh does not generate the notices"

echo "--- an unaccepted license fails packaging"
printf 'accepted = ["MIT"]\n' >"$work/narrow.toml"
rm -rf "$work/failing" && mkdir "$work/failing"
if FUGUE_ABOUT_CONFIG="$work/narrow.toml" bash "$ROOT/scripts/package-native.sh" \
  x86_64-unknown-linux-gnu "$VERSION" "$work/failing" >/dev/null 2>"$work/failing.err"; then
  fail "packaging passed with a dependency license that is not accepted"
elif ! grep -q 'could not generate the notices' "$work/failing.err"; then
  fail "packaging failed, but not because the notices could not be generated"
  cat "$work/failing.err" >&2
elif ls "$work/failing"/*.tar.gz >/dev/null 2>&1; then
  fail "a failed notices build still left an archive"
else
  pass "packaging fails, and leaves no archive, when a dependency license is not accepted"
fi

echo "--- the accepted licenses match deny.toml"
accepted() { python3 - "$1" "$2" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
block = re.search(rf"^{sys.argv[2]}\s*=\s*\[(.*?)\]", text, re.S | re.M).group(1)
print("\n".join(sorted(re.findall(r'"([^"]+)"', block))))
PY
}
if diff <(accepted "$ROOT/packaging/licenses/about.toml" accepted) <(accepted "$ROOT/deny.toml" allow) >/dev/null; then
  pass "packaging/licenses/about.toml accepts exactly the licenses deny.toml allows"
else
  fail "packaging/licenses/about.toml and deny.toml disagree on the accepted licenses"
fi

exit "$failed"
