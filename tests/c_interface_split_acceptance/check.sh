#!/usr/bin/env bash
# Acceptance checks for moving core's C interface into its own workspace crate.
#
# The fugue crate is what Rust consumers (the product) link, so building it
# must no longer produce the C libraries; a separate crate in this workspace
# produces them instead, with the same exported C symbols, the same header and
# the same release archives as main. These are build-level properties, so they
# live here rather than in `cargo test`: each check builds with cargo.
#
#   C1  Building the fugue crate alone produces no libfugue.a, libfugue.dylib,
#       libfugue.so or fugue.dll.
#   C2  A crate in the workspace produces the static and dynamic libraries; its
#       exported C symbols match main's (golden/c-api.txt, and for this host
#       and toolchain golden/static-unmangled-<host>.txt) and include/fugue.h
#       is unchanged (golden/fugue.h.sha256).
#   C3  package-native.sh, package-android.sh and package-ios.sh produce
#       archives with main's names and contents. Libraries are compared by
#       their exported symbols, since rebuilds are not byte-identical.
#
# The fyug-product half (no C libraries in its target/) is check-product.sh.
# The golden files were captured from main by capture-golden.sh; see there.
#
# Usage (from the repo root): tests/c_interface_split_acceptance/check.sh
#
# Environment:
#   CARGO_TARGET_DIR        cargo's target directory (default: ./target)
#   FUGUE_BASELINE_LIB_DIR  optional directory holding main's libfugue.a and
#                           shared library for this host; when set, C2 also
#                           compares the full static symbol set against it
#
# Needs: cargo, rustc, nm, tar, python3. The archive notices need cargo-about
# 0.9.3; without it a stand-in writes placeholder notices, the layout is still
# checked, and scripts/check-release-notices.sh (the real notices) is skipped.
# Android and iOS builds run only when their Rust targets are installed;
# otherwise they are skipped (the Android layout is still checked with stub
# libraries, as check-release-notices.sh does).
#
# Prints PASS, FAIL or SKIP per check and exits non-zero if any check failed.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
GOLDEN="$HERE/golden"
source "$HERE/lib.sh"
cd "$ROOT"

HOST="$(host_triple)"
TD="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR="$TD"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
GOLDEN_HOST="$(cat "$GOLDEN/host")"
case "$(uname -s)" in Darwin) SHARED=libfugue.dylib ;; *) SHARED=libfugue.so ;; esac
C_LIBS=(libfugue.a libfugue.dylib libfugue.so fugue.dll fugue.lib fugue.dll.lib)

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failed=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; failed=1; }
skip() { echo "SKIP: $1"; }

# same <label> <expected-file> <actual-file>: pass when the files are equal,
# otherwise fail and show the difference.
same() {
  if diff -u "$2" "$3" >"$work/diff" 2>&1; then
    pass "$1"
  else
    fail "$1"
    sed 's/^/    /' "$work/diff" | head -n 40
  fi
}

# same_sha <label> <golden-sha-file> <file>
same_sha() {
  if [[ -f "$3" && "$(sha "$3")" == "$(cat "$2")" ]]; then pass "$1"; else fail "$1"; fi
}

# --- workspace layout -------------------------------------------------------

cargo metadata --no-deps --format-version 1 >"$work/metadata.json"
# Prints "<name>\t<manifest>\t<has-ffi-feature>\t<crate-types>" per package.
python3 - "$work/metadata.json" >"$work/packages.tsv" <<'PY'
import json, sys
meta = json.load(open(sys.argv[1]))
members = set(meta["workspace_members"])
for p in meta["packages"]:
    if p["id"] not in members:
        continue
    types = sorted({t for tg in p["targets"] for t in tg["crate_types"]
                    if set(tg["kind"]) & {"lib", "rlib", "staticlib", "cdylib", "dylib"}})
    print("\t".join([p["name"], p["manifest_path"], "ffi" if "ffi" in p["features"] else "-", ",".join(types)]))
PY

FUGUE_MANIFEST="$(awk -F'\t' '$1 == "fugue" {print $2}' "$work/packages.tsv")"
FUGUE_FFI="$(awk -F'\t' '$1 == "fugue" {print $3}' "$work/packages.tsv")"
FUGUE_TYPES="$(awk -F'\t' '$1 == "fugue" {print $4}' "$work/packages.tsv")"
# The C interface crate: the workspace member whose library is both a
# staticlib and a cdylib (on main, that is fugue itself).
C_LINE="$(awk -F'\t' '$4 ~ /staticlib/ && $4 ~ /cdylib/' "$work/packages.tsv" | head -n 1)"
C_PKG="$(cut -f1 <<<"$C_LINE")"
C_MANIFEST="$(cut -f2 <<<"$C_LINE")"
C_FFI="$(cut -f3 <<<"$C_LINE")"

echo "host $HOST, target dir $TD, version $VERSION"
echo "fugue crate types: $FUGUE_TYPES; C interface crate: ${C_PKG:-none}"

# artifacts <json> <manifest>: the files cargo reported for that package's
# library, one per line.
artifacts() {
  python3 - "$1" "$2" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    if msg.get("reason") == "compiler-artifact" and msg.get("manifest_path") == sys.argv[2]:
        for f in msg.get("filenames", []):
            print(f)
PY
}

remove_c_libs() {
  local lib
  for lib in "${C_LIBS[@]}"; do rm -f "$TD/$HOST/release/$lib" "$TD/release/$lib"; done
}

# --- C1 ---------------------------------------------------------------------

echo "--- C1: building the fugue crate alone produces no C libraries"
if [[ "$FUGUE_TYPES" == *staticlib* || "$FUGUE_TYPES" == *cdylib* ]]; then
  fail "C1 fugue's library declares no staticlib or cdylib crate type (declares $FUGUE_TYPES)"
else
  pass "C1 fugue's library declares no staticlib or cdylib crate type"
fi

c1_variants=("")
if [[ "$FUGUE_FFI" == "ffi" ]]; then c1_variants+=("ffi"); else skip "C1 fugue has no ffi feature, so only its default build is checked"; fi
for features in "${c1_variants[@]}"; do
  label="cargo build --release -p fugue${features:+ --features $features}"
  remove_c_libs
  args=(build --release -p fugue --target "$HOST" --message-format=json-render-diagnostics)
  [[ -n "$features" ]] && args+=(--features "$features")
  if ! cargo "${args[@]}" >"$work/c1.json"; then
    fail "C1 $label builds"
    continue
  fi
  reported="$(artifacts "$work/c1.json" "$FUGUE_MANIFEST" | grep -E '\.(a|dylib|so|dll|lib)$' || true)"
  if [[ -n "$reported" ]]; then
    fail "C1 $label reports no C library artifacts (got: $(tr '\n' ' ' <<<"$reported"))"
  else
    pass "C1 $label reports no C library artifacts"
  fi
  found=()
  for lib in "${C_LIBS[@]}"; do
    [[ -e "$TD/$HOST/release/$lib" ]] && found+=("$lib")
  done
  if [[ ${#found[@]} -gt 0 ]]; then
    fail "C1 $label leaves none of the C libraries in $TD/$HOST/release (found: ${found[*]})"
  else
    pass "C1 $label leaves none of the C libraries in $TD/$HOST/release"
  fi
done

# --- C2 ---------------------------------------------------------------------

echo "--- C2: a workspace crate produces the C libraries with main's symbols"
same_sha "C2 include/fugue.h is unchanged from main" "$GOLDEN/fugue.h.sha256" "$ROOT/include/fugue.h"

LIBS="$work/libs"
mkdir -p "$LIBS"
have_libs=0
if [[ -z "$C_PKG" ]]; then
  fail "C2 a workspace member builds a staticlib and a cdylib"
else
  pass "C2 a workspace member builds a staticlib and a cdylib ($C_PKG)"
  [[ "$C_PKG" == "fugue" ]] && echo "    note: that is the fugue crate itself, so the C interface has not moved"
  args=(build --release -p "$C_PKG" --target "$HOST" --message-format=json-render-diagnostics)
  [[ "$C_FFI" == "ffi" ]] && args+=(--features ffi)
  remove_c_libs
  if ! cargo "${args[@]}" >"$work/c2.json"; then
    fail "C2 cargo ${args[*]} builds"
  else
    static="$(artifacts "$work/c2.json" "$C_MANIFEST" | grep -E '\.a$' | head -n 1 || true)"
    shared="$(artifacts "$work/c2.json" "$C_MANIFEST" | grep -E '\.(dylib|so)$' | head -n 1 || true)"
    if [[ -n "$static" && -n "$shared" ]]; then
      pass "C2 $C_PKG produces a static and a dynamic library"
      cp "$static" "$LIBS/libfugue.a"
      cp "$shared" "$LIBS/$SHARED"
      have_libs=1
      if [[ "$(basename "$static")" == libfugue.a && "$(basename "$shared")" == "$SHARED" ]]; then
        pass "C2 the libraries keep main's names (libfugue.a, $SHARED)"
      else
        fail "C2 the libraries keep main's names (got $(basename "$static"), $(basename "$shared"))"
      fi
    else
      fail "C2 $C_PKG produces a static and a dynamic library (got: $(artifacts "$work/c2.json" "$C_MANIFEST" | tr '\n' ' '))"
    fi
  fi
fi

# check_lib_symbols <label> <static-lib-or-empty> <shared-lib-or-empty>
check_lib_symbols() {
  local label="$1" static="$2" shared="$3"
  if [[ -n "$shared" ]]; then
    dylib_exports "$shared" >"$work/exports.txt"
    same "$label the dynamic library exports exactly main's C symbols" "$GOLDEN/c-api.txt" "$work/exports.txt"
  fi
  if [[ -n "$static" ]]; then
    static_unmangled "$static" >"$work/static.txt"
    c_api "$work/static.txt" >"$work/static-api.txt"
    same "$label the static library defines exactly main's fugue_* C symbols" "$GOLDEN/c-api.txt" "$work/static-api.txt"
  fi
}

if [[ $have_libs -eq 1 ]]; then
  check_lib_symbols "C2" "$LIBS/libfugue.a" "$LIBS/$SHARED"
  golden_static="$GOLDEN/static-unmangled-$HOST.txt"
  toolchain="$(rustc -V; nm --version 2>/dev/null | grep -i version | head -n 1)"
  if [[ -f "$golden_static" && "$toolchain" == "$(cat "$GOLDEN/static-unmangled-$HOST.toolchain")" ]]; then
    static_unmangled "$LIBS/libfugue.a" >"$work/static-all.txt"
    same "C2 the static library's unmangled C symbols match main's" "$golden_static" "$work/static-all.txt"
  else
    skip "C2 full static symbol set: the golden list is for $(tr '\n' ' ' <"$GOLDEN/static-unmangled-$GOLDEN_HOST.toolchain")on $GOLDEN_HOST (set FUGUE_BASELINE_LIB_DIR to compare against main's libraries built here)"
  fi
  if [[ -n "${FUGUE_BASELINE_LIB_DIR:-}" ]]; then
    static_unmangled "$FUGUE_BASELINE_LIB_DIR/libfugue.a" >"$work/base-static.txt"
    static_unmangled "$LIBS/libfugue.a" >"$work/static-all.txt"
    same "C2 the static library's unmangled C symbols match the baseline in FUGUE_BASELINE_LIB_DIR" "$work/base-static.txt" "$work/static-all.txt"
    dylib_exports "$FUGUE_BASELINE_LIB_DIR/$SHARED" >"$work/base-exports.txt"
    dylib_exports "$LIBS/$SHARED" >"$work/exports.txt"
    same "C2 the dynamic library's exports match the baseline in FUGUE_BASELINE_LIB_DIR" "$work/base-exports.txt" "$work/exports.txt"
  fi
fi

# --- C3 ---------------------------------------------------------------------

echo "--- C3: the packaging scripts produce main's archives"
if have_real_cargo_about; then
  real_about=1
else
  real_about=0
  install_cargo_about_stub "$work/bin"
  echo "    note: cargo-about 0.9.3 is not installed; archives carry placeholder notices"
fi

# check_common <label> <extracted-top-dir>: LICENSE and fugue.h are main's.
check_common() {
  same_sha "$1 LICENSE is main's" "$GOLDEN/LICENSE.sha256" "$2/LICENSE"
  same_sha "$1 fugue.h is main's" "$GOLDEN/fugue.h.sha256" "$2/fugue.h"
}

# Native archive for the host, from the libraries C2 built where the release
# workflow leaves them.
dist="$work/dist"
mkdir -p "$dist"
if [[ $have_libs -eq 0 ]]; then
  fail "C3 package-native.sh: no libraries to package (C2 did not build them)"
elif ! FUGUE_TARGET_DIR="$TD" bash scripts/package-native.sh "$HOST" "$VERSION" "$dist" >"$work/native.log" 2>&1; then
  fail "C3 package-native.sh $HOST succeeds"
  sed 's/^/    /' "$work/native.log" | tail -n 20
else
  archive="$dist/fugue-$VERSION-$HOST.tar.gz"
  if [[ -f "$archive" ]]; then pass "C3 package-native.sh produces fugue-$VERSION-$HOST.tar.gz"; else fail "C3 package-native.sh produces fugue-$VERSION-$HOST.tar.gz"; fi
  if [[ -f "$archive" ]]; then
    top="$work/native/fugue-$VERSION-$HOST"
    mkdir -p "$work/native" && tar -xzf "$archive" -C "$work/native"
    if [[ "$HOST" == "$GOLDEN_HOST" ]]; then
      archive_listing "$archive" "$VERSION" "$HOST" >"$work/listing.txt"
      same "C3 native archive lists main's files" "$GOLDEN/native-listing.txt" "$work/listing.txt"
      normalize "$top/README.md" "$VERSION" "$HOST" >"$work/readme.txt"
      same "C3 native README.md is main's" "$GOLDEN/native-README.md" "$work/readme.txt"
      if [[ $real_about -eq 0 ]]; then
        normalize "$top/THIRD_PARTY_LICENSES.txt" "$VERSION" "$HOST" >"$work/notices.txt"
        same "C3 native THIRD_PARTY_LICENSES.txt wrapper is main's (placeholder crate list)" "$GOLDEN/native-notices-stub-body.txt" "$work/notices.txt"
      else
        skip "C3 native notices crate list: generated by the real cargo-about; checked by check-release-notices.sh below"
      fi
    else
      skip "C3 native listing and README: the golden files are for $GOLDEN_HOST"
    fi
    check_common "C3 native archive:" "$top"
    check_lib_symbols "C3 native archive:" "$top/lib/libfugue.a" "$top/lib/$SHARED"
  fi
fi

# Android layout, from stub libraries (as check-release-notices.sh does).
stubs="$work/stub-target"
for t in aarch64-linux-android x86_64-linux-android; do
  mkdir -p "$stubs/$t/release" && echo stub >"$stubs/$t/release/libfugue.so"
done
if FUGUE_TARGET_DIR="$stubs" bash scripts/package-android.sh "$VERSION" "$work/android-dist" >"$work/android.log" 2>&1 \
  && [[ -f "$work/android-dist/fugue-$VERSION-android.tar.gz" ]]; then
  pass "C3 package-android.sh produces fugue-$VERSION-android.tar.gz (stub libraries)"
  archive="$work/android-dist/fugue-$VERSION-android.tar.gz"
  archive_listing "$archive" "$VERSION" android >"$work/listing.txt"
  same "C3 android archive lists main's files" "$GOLDEN/android-listing.txt" "$work/listing.txt"
  mkdir -p "$work/android" && tar -xzf "$archive" -C "$work/android"
  normalize "$work/android/fugue-$VERSION-android/README.md" "$VERSION" android >"$work/readme.txt"
  same "C3 android README.md is main's" "$GOLDEN/android-README.md" "$work/readme.txt"
  check_common "C3 android archive:" "$work/android/fugue-$VERSION-android"
else
  fail "C3 package-android.sh produces fugue-$VERSION-android.tar.gz (stub libraries)"
  sed 's/^/    /' "$work/android.log" | tail -n 20
fi

installed_targets="$(rustup target list --installed 2>/dev/null || true)"
has_targets() {
  local t
  for t in "$@"; do grep -qx "$t" <<<"$installed_targets" || return 1; done
}

# Android, built for real, when its targets (and an NDK linker) are set up.
if [[ -n "$C_PKG" ]] && has_targets aarch64-linux-android x86_64-linux-android; then
  ok=1
  for t in aarch64-linux-android x86_64-linux-android; do
    args=(build --release -p "$C_PKG" --target "$t")
    [[ "$C_FFI" == "ffi" ]] && args+=(--features ffi)
    cargo "${args[@]}" >"$work/android-build.log" 2>&1 || { ok=0; fail "C3 cargo ${args[*]} builds"; sed 's/^/    /' "$work/android-build.log" | tail -n 10; }
  done
  if [[ $ok -eq 1 ]]; then
    rm -rf "$work/android-real" && mkdir -p "$work/android-real"
    if FUGUE_TARGET_DIR="$TD" bash scripts/package-android.sh "$VERSION" "$work/android-real" >"$work/android.log" 2>&1; then
      archive="$work/android-real/fugue-$VERSION-android.tar.gz"
      archive_listing "$archive" "$VERSION" android >"$work/listing.txt"
      same "C3 android archive (real build) lists main's files" "$GOLDEN/android-listing.txt" "$work/listing.txt"
      mkdir -p "$work/android-x" && tar -xzf "$archive" -C "$work/android-x"
      for abi in arm64-v8a x86_64; do
        nm -D --defined-only "$work/android-x/fugue-$VERSION-android/lib/$abi/libfugue.so" 2>/dev/null | awk 'NF >= 3 {print $3}' | sort -u >"$work/exports.txt" || true
        same "C3 android $abi libfugue.so exports exactly main's C symbols" "$GOLDEN/c-api.txt" "$work/exports.txt"
      done
    else
      fail "C3 package-android.sh succeeds with real libraries"
      sed 's/^/    /' "$work/android.log" | tail -n 20
    fi
  fi
else
  skip "C3 android real build: the aarch64-linux-android and x86_64-linux-android targets are not installed"
fi

# iOS needs its targets and xcodebuild. package-ios.sh reads ./target.
ios_targets=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
if [[ -n "$C_PKG" ]] && has_targets "${ios_targets[@]}" && command -v xcodebuild >/dev/null; then
  ok=1
  for t in "${ios_targets[@]}"; do
    args=(build --release -p "$C_PKG" --target "$t")
    [[ "$C_FFI" == "ffi" ]] && args+=(--features ffi)
    CARGO_TARGET_DIR="$ROOT/target" cargo "${args[@]}" >"$work/ios-build.log" 2>&1 || { ok=0; fail "C3 cargo ${args[*]} builds"; sed 's/^/    /' "$work/ios-build.log" | tail -n 10; }
  done
  if [[ $ok -eq 1 ]]; then
    mkdir -p "$work/ios-dist"
    if FUGUE_TARGET_DIR="$ROOT/target" bash scripts/package-ios.sh "$VERSION" "$work/ios-dist" >"$work/ios.log" 2>&1 \
      && [[ -f "$work/ios-dist/fugue-$VERSION-ios-xcframework.tar.gz" ]]; then
      pass "C3 package-ios.sh produces fugue-$VERSION-ios-xcframework.tar.gz"
      mkdir -p "$work/ios" && tar -xzf "$work/ios-dist/fugue-$VERSION-ios-xcframework.tar.gz" -C "$work/ios"
      top="$work/ios/fugue-$VERSION-ios"
      for entry in Fugue.xcframework/Info.plist LICENSE README.md THIRD_PARTY_LICENSES.txt; do
        [[ -e "$top/$entry" ]] && pass "C3 iOS archive holds $entry" || fail "C3 iOS archive holds $entry"
      done
      same_sha "C3 iOS archive: LICENSE is main's" "$GOLDEN/LICENSE.sha256" "$top/LICENSE"
      sed "s|$VERSION|<VERSION>|g" "$top/README.md" >"$work/readme.txt"
      same "C3 iOS README.md is main's" "$GOLDEN/ios-README.md" "$work/readme.txt"
      slices=0
      while IFS= read -r lib; do
        slices=$((slices + 1))
        static_unmangled "$lib" | c_api >"$work/static-api.txt"
        same "C3 iOS slice $(basename "$(dirname "$lib")") libfugue.a defines exactly main's fugue_* C symbols" "$GOLDEN/c-api.txt" "$work/static-api.txt"
      done < <(find "$top/Fugue.xcframework" -name libfugue.a)
      [[ $slices -eq 2 ]] && pass "C3 iOS xcframework holds a device and a simulator libfugue.a" || fail "C3 iOS xcframework holds a device and a simulator libfugue.a (found $slices)"
      while IFS= read -r header; do
        same_sha "C3 iOS header $(basename "$(dirname "$(dirname "$header")")") fugue.h is main's" "$GOLDEN/fugue.h.sha256" "$header"
      done < <(find "$top/Fugue.xcframework" -name fugue.h)
    else
      fail "C3 package-ios.sh produces fugue-$VERSION-ios-xcframework.tar.gz"
      sed 's/^/    /' "$work/ios.log" | tail -n 20
    fi
  fi
else
  skip "C3 iOS archive: needs xcodebuild and the ${ios_targets[*]} targets"
fi

if [[ $real_about -eq 1 ]]; then
  if bash scripts/check-release-notices.sh >"$work/notices.log" 2>&1; then
    pass "C3 scripts/check-release-notices.sh passes"
  else
    fail "C3 scripts/check-release-notices.sh passes"
    grep -E '^FAIL|error' "$work/notices.log" | sed 's/^/    /' | head -n 20
  fi
else
  skip "C3 scripts/check-release-notices.sh: needs cargo-about 0.9.3 (cargo install --locked cargo-about --version 0.9.3 --features cli)"
fi

exit "$failed"
