#!/bin/sh
# Test install.sh against a local archive, with no network: a fake `curl` on
# PATH serves the archive from a temp directory. Checks that the license notices
# land in FUGUE_DOC_DIR beside the installed binaries, and that an archive
# without them still installs the binaries and warns.
#
# Usage: scripts/test-install.sh

set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-gnu" ;;
  *) echo "SKIP: no prebuilt target for $(uname -s)-$(uname -m)"; exit 0 ;;
esac

mkdir -p "$WORK/fake" "$WORK/serve"
cat >"$WORK/fake/curl" <<'CURL'
#!/bin/sh
# Serve $SERVE_DIR/<asset basename> for the URL, writing it to the -o file.
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
[ -f "$SERVE_DIR/${url##*/}" ] || exit 22
cp "$SERVE_DIR/${url##*/}" "$out"
CURL
chmod +x "$WORK/fake/curl"

# build_archive <dir> <with-notices:0|1>
build_archive() {
  stage="$WORK/stage"
  rm -rf "$stage" && mkdir -p "$stage"
  for b in fugue fugue-mcp; do
    printf '#!/bin/sh\necho stub\n' >"$stage/$b"
    chmod +x "$stage/$b"
  done
  if [ "$2" = 1 ]; then
    printf 'THIRD-PARTY SOFTWARE NOTICES AND LICENSES\n' >"$stage/THIRD_PARTY_LICENSES.txt"
  fi
  rm -f "$1/fugue-tools-$target.tar.gz"
  tar -C "$stage" -czf "$1/fugue-tools-$target.tar.gz" .
}

failed=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failed=1; }

run_install() {
  # run_install <name>: install into $WORK/<name>/{bin,doc}; log to $WORK/<name>.log
  SERVE_DIR="$WORK/serve" PATH="$WORK/fake:$PATH" FUGUE_VERSION=2026.0.0 \
    FUGUE_BIN_DIR="$WORK/$1/bin" FUGUE_DOC_DIR="$WORK/$1/doc" \
    sh "$ROOT/install.sh" >"$WORK/$1.log" 2>&1
}

# --- an archive that carries the notices
build_archive "$WORK/serve" 1
if run_install with; then pass "install succeeds with notices in the archive"; else fail "install failed"; cat "$WORK/with.log" >&2; fi
[ -x "$WORK/with/bin/fugue" ] && [ -x "$WORK/with/bin/fugue-mcp" ] && pass "binaries installed" || fail "binaries missing"
[ -s "$WORK/with/doc/THIRD_PARTY_LICENSES.txt" ] && pass "notices installed to FUGUE_DOC_DIR" || fail "notices not in FUGUE_DOC_DIR"
grep -qF "Installed license notices → $WORK/with/doc/THIRD_PARTY_LICENSES.txt" "$WORK/with.log" \
  && pass "installer prints where the notices went" || { fail "installer did not print the notices path"; cat "$WORK/with.log" >&2; }

# --- the default location follows HOME, beside the default bin directory
default_home="$WORK/home"
mkdir -p "$default_home"
if SERVE_DIR="$WORK/serve" PATH="$WORK/fake:$PATH" FUGUE_VERSION=2026.0.0 HOME="$default_home" \
  sh "$ROOT/install.sh" >"$WORK/default.log" 2>&1; then
  [ -s "$default_home/.fugue/share/doc/THIRD_PARTY_LICENSES.txt" ] \
    && pass "notices default to ~/.fugue/share/doc" || fail "notices not in ~/.fugue/share/doc"
else
  fail "install with default locations failed"; cat "$WORK/default.log" >&2
fi

# --- an archive without them: warn, but still install the binaries
build_archive "$WORK/serve" 0
if run_install without; then pass "install succeeds without notices in the archive"; else fail "install failed without notices"; cat "$WORK/without.log" >&2; fi
[ -x "$WORK/without/bin/fugue" ] && [ -x "$WORK/without/bin/fugue-mcp" ] && pass "binaries still installed" || fail "binaries missing"
grep -q "warning:.*THIRD_PARTY_LICENSES.txt" "$WORK/without.log" && pass "missing notices warn" || fail "no warning for missing notices"
[ ! -e "$WORK/without/doc/THIRD_PARTY_LICENSES.txt" ] && pass "no notices file invented" || fail "notices file exists"

exit "$failed"
