#!/usr/bin/env bash
# Smoke test for scripts/package-mcpb.sh (FUG-232).
#
# The `.mcpb` is only assembled during a release, from artifacts that exist only
# then, so nothing else exercises the packager until a release is already in
# flight. This builds stub install units and runs the real script over them, so
# a broken manifest template, a renamed placeholder, or a lost executable bit
# fails on the pull request instead.
#
# Usage: test-package-mcpb.sh

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/tools" "$work/out"

# One bundled target per platform class: macOS and Windows are bundled, Linux is
# expected to be skipped because Claude Desktop does not run there.
make_unit() {
  # $1 = target triple, $2.. = binary names
  local target="$1"
  shift
  local stage
  stage="$(mktemp -d)"
  for bin in "$@"; do
    printf '#!/bin/sh\necho stub\n' > "$stage/$bin"
    chmod +x "$stage/$bin"
  done
  tar -C "$stage" -czf "$work/tools/fugue-tools-${target}.tar.gz" .
  rm -rf "$stage"
}

make_unit aarch64-apple-darwin fugue fugue-mcp
make_unit x86_64-pc-windows-msvc fugue.exe fugue-mcp.exe
make_unit x86_64-unknown-linux-gnu fugue fugue-mcp

version="0.0.0-test"
bash "$repo_root/scripts/package-mcpb.sh" "$work/tools" "$work/out" "$version"

fail() { echo "FAIL: $1" >&2; exit 1; }

[[ -f "$work/out/fugue-${version}-aarch64-apple-darwin.mcpb" ]] \
  || fail "no macOS bundle produced"
[[ -f "$work/out/fugue-${version}-x86_64-pc-windows-msvc.mcpb" ]] \
  || fail "no Windows bundle produced"
[[ ! -e "$work/out/fugue-${version}-x86_64-unknown-linux-gnu.mcpb" ]] \
  || fail "produced a Linux bundle; Claude Desktop does not run on Linux"

# Unpacking restores the recorded permission bits, so this asserts the server
# binary is still executable after the zip round-trip — a bundle whose server
# cannot be executed would otherwise fail only once a user installs it.
unpacked="$work/unpacked"
npx --yes "@anthropic-ai/mcpb@${MCPB_CLI_VERSION:-2.1.2}" unpack \
  "$work/out/fugue-${version}-aarch64-apple-darwin.mcpb" "$unpacked" >/dev/null

[[ -x "$unpacked/server/fugue-mcp" ]] || fail "server/fugue-mcp is not executable"
[[ -x "$unpacked/server/fugue" ]] || fail "sibling server/fugue is not executable"

manifest="$unpacked/manifest.json"
[[ -f "$manifest" ]] || fail "bundle has no manifest.json at its root"

# No placeholder may survive substitution.
if grep -q '__[A-Z_]*__' "$manifest"; then
  grep -o '__[A-Z_]*__' "$manifest" >&2
  fail "manifest still contains unsubstituted placeholders"
fi

check_field() {
  # $1 = jq filter, $2 = expected value
  local actual
  actual="$(jq -r "$1" "$manifest")"
  [[ "$actual" == "$2" ]] || fail "manifest $1 is '$actual', expected '$2'"
}

check_field '.version' "$version"
check_field '.compatibility.platforms | join(",")' 'darwin'
check_field '.server.type' 'binary'
check_field '.server.entry_point' 'server/fugue-mcp'
# Mirrors the Claude Desktop registration `fugue setup` writes (FUG-229):
# the MCP binary as the command, with no arguments.
# shellcheck disable=SC2016  # ${__dirname} is an MCPB manifest variable, not shell.
check_field '.server.mcp_config.command' '${__dirname}/server/fugue-mcp'
check_field '.server.mcp_config.args | length' '0'

win_manifest="$work/win-manifest"
npx --yes "@anthropic-ai/mcpb@${MCPB_CLI_VERSION:-2.1.2}" unpack \
  "$work/out/fugue-${version}-x86_64-pc-windows-msvc.mcpb" "$win_manifest" >/dev/null
actual="$(jq -r '.server.mcp_config.command' "$win_manifest/manifest.json")"
# shellcheck disable=SC2016  # ${__dirname} is an MCPB manifest variable, not shell.
[[ "$actual" == '${__dirname}/server/fugue-mcp.exe' ]] \
  || fail "Windows manifest command is '$actual', expected the .exe path"

echo "PASS: package-mcpb.sh produces valid bundles"
