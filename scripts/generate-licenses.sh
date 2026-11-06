#!/usr/bin/env bash
# Write the third-party notices a library release archive ships with.
#
# MIT, Apache-2.0 and the other licenses in the dependency tree require their
# text and copyright notices to travel with the compiled library. This generates
# them from the dependency tree Cargo resolves for the archive's targets, so the
# file cannot go stale: cargo-about reads each crate's own license files, and
# fails on a crate whose license is not accepted in packaging/licenses/about.toml.
# Every package-*.sh calls it, so a release cannot publish an archive without
# the notices: when generation fails, packaging fails.
#
# Usage: generate-licenses.sh <out-file> <target>...
#        generate-licenses.sh --cargo-about-version

set -euo pipefail

# The generator version. CI installs exactly this one (release.yml reads it with
# --cargo-about-version), as the product release does.
CARGO_ABOUT_VERSION=0.9.3

if [[ "${1:-}" == "--cargo-about-version" ]]; then
  echo "$CARGO_ABOUT_VERSION"
  exit 0
fi

die() { echo "error: $*" >&2; exit 1; }

OUT_FILE="${1:?output file required}"
shift
[[ $# -gt 0 ]] || die "at least one target triple is required"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LICENSES_DIR="$ROOT/packaging/licenses"
# Only scripts/check-release-notices.sh overrides the config, to prove a license
# that is not accepted fails the build.
ABOUT_CONFIG="${FUGUE_ABOUT_CONFIG:-$LICENSES_DIR/about.toml}"

have="$(cargo about --version 2>/dev/null | awk '{print $2}')" || true
[[ "$have" == "$CARGO_ABOUT_VERSION" ]] \
  || die "cargo-about $CARGO_ABOUT_VERSION is required (found '${have:-none}'); install it with: cargo install --locked cargo-about --version $CARGO_ABOUT_VERSION --features cli"

targets=()
for target in "$@"; do
  targets+=(--target "$target")
done

body="$(mktemp)"
log="$(mktemp)"
trap 'rm -f "$body" "$log"' EXIT
(
  cd "$ROOT"
  cargo about generate --locked --fail "${targets[@]}" \
    --config "$ABOUT_CONFIG" --output-file "$body" "$LICENSES_DIR/about.hbs"
) 2>"$log" || { cat "$log" >&2; die "cargo-about could not generate the notices for $*"; }
cat "$log" >&2

# --fail stops on an unaccepted license. A custom license with no file in the
# crate has no standard text to fall back on, and cargo-about only warns. Treat
# that as a failure: clarify the crate in about.toml.
if grep -q 'has no license file' "$log"; then
  die "a crate has no license file for its license (see the warnings above); clarify it in packaging/licenses/about.toml"
fi

mkdir -p "$(dirname "$OUT_FILE")"
{
  printf 'THIRD-PARTY SOFTWARE NOTICES AND LICENSES\n\n'
  printf 'Fugue %s library for %s.\n\n' "$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)" "$*"
  cat "$body"
  # Source ported into core carries its own licenses (see docs/dsp-provenance.md).
  if [[ -f "$ROOT/THIRD_PARTY_NOTICES.md" ]]; then
    printf '\n================================================================================\n'
    printf 'SOURCE PORTED INTO FUGUE\n\n'
    cat "$ROOT/THIRD_PARTY_NOTICES.md"
  fi
} > "$OUT_FILE"

# Guard the file a release ships: it names Fugue's own crate and carries the MIT
# and Apache texts nearly every dependency relies on.
grep -Eq '^  fugue [0-9]' "$OUT_FILE" || die "$OUT_FILE does not list the fugue crate"
for required in 'Permission is hereby granted, free of charge' 'Apache License' 'Version 2.0, January 2004'; do
  grep -qF "$required" "$OUT_FILE" || die "$OUT_FILE lacks '$required'"
done

echo "Wrote $OUT_FILE ($(grep -c '^  [^ ]* [0-9][^ ]* (' "$OUT_FILE") crates)"
