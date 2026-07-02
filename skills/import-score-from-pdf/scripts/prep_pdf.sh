#!/usr/bin/env sh
#
# prep_pdf.sh — deterministic PDF preprocessing for the import-score-from-pdf skill.
# POSIX sh; runs on macOS, Linux, WSL, and Git Bash. Windows users without a POSIX
# shell should use the sibling prep_pdf.ps1 (identical behavior).
#
# Renders a score PDF's pages to PNGs at a fixed DPI and extracts text/metadata
# anchors using poppler, then writes a manifest.json. This is the agent-agnostic
# core of the skill: any harness (Claude Code, Codex, the in-graph Agent module,
# a human) can run it to get an identical, inspectable view of a score PDF.
#
# We deliberately do NOT rasterize PDFs ourselves — poppler is the renderer the
# surrounding toolchain already assumes, and pinning it (version recorded in the
# manifest) is how we get determinism. See the decision ledger entry
# .harness/decisions/2026-06-30-judgment.md.
#
# Usage:
#   prep_pdf.sh [--dpi N] [--install] <input.pdf> <output_dir>
#
#   --dpi N     Render resolution in DPI (default: 200). Pinned for reproducibility.
#   --install   If poppler is missing, run the platform install command instead of
#               only printing it. (Install mutates your system — see preflight.)
#
# Output (in <output_dir>):
#   page-1.png, page-2.png, ...   rendered pages at the pinned DPI
#   info.txt                      pdfinfo output (Title, Author, Creator, pages, size)
#   text.txt                      pdftotext -layout output (embedded text streams)
#   manifest.json                 poppler version, dpi, page list, anchor files
#
set -eu

DEFAULT_DPI=200
DPI="$DEFAULT_DPI"
INSTALL=0
INPUT=""
OUTDIR=""

die() { printf 'prep_pdf: %s\n' "$1" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dpi) DPI="${2:-}"; shift 2 ;;
    --install) INSTALL=1; shift ;;
    -h|--help) sed -n '2,32p' "$0"; exit 0 ;;
    -*) die "unknown option: $1" ;;
    *)
      if [ -z "$INPUT" ]; then INPUT="$1"
      elif [ -z "$OUTDIR" ]; then OUTDIR="$1"
      else die "unexpected argument: $1"; fi
      shift ;;
  esac
done

[ -n "$INPUT" ]  || die "missing <input.pdf> (see --help)"
[ -n "$OUTDIR" ] || die "missing <output_dir> (see --help)"
[ -f "$INPUT" ]  || die "input PDF not found: $INPUT"
case "$DPI" in *[!0-9]*|'') die "--dpi must be a positive integer" ;; esac

# --- Preflight: ensure poppler (pdftocairo, pdftotext, pdfinfo) --------------
# Prints the platform install command; only runs it with --install. Windows
# managers (winget/choco/scoop) are handled by prep_pdf.ps1.
install_cmd() {
  if command -v brew >/dev/null 2>&1; then
    echo "brew install poppler"
  elif command -v apt-get >/dev/null 2>&1; then
    echo "sudo apt-get update && sudo apt-get install -y poppler-utils"
  elif command -v dnf >/dev/null 2>&1; then
    echo "sudo dnf install -y poppler-utils"
  elif command -v zypper >/dev/null 2>&1; then
    echo "sudo zypper install -y poppler-tools"
  elif command -v pacman >/dev/null 2>&1; then
    echo "sudo pacman -S --noconfirm poppler"
  elif command -v apk >/dev/null 2>&1; then
    echo "sudo apk add poppler-utils"
  else
    echo ""
  fi
}

missing_tools=""
for tool in pdftocairo pdftotext pdfinfo; do
  command -v "$tool" >/dev/null 2>&1 || missing_tools="$missing_tools $tool"
done

if [ -n "$missing_tools" ]; then
  cmd="$(install_cmd)"
  [ -n "$cmd" ] || die "poppler is required (missing:$missing_tools) but no supported package manager was found. Install poppler/poppler-utils manually."
  if [ "$INSTALL" -eq 1 ]; then
    printf 'prep_pdf: poppler missing (%s) — installing with:\n  %s\n' "$missing_tools" "$cmd" >&2
    sh -c "$cmd" >&2 || die "poppler install failed; run it manually: $cmd"
  else
    die "poppler is required (missing:$missing_tools). Install it, then re-run:
  $cmd
or re-run this script with --install to do it for you."
  fi
fi

POPPLER_VERSION="$(pdftocairo -v 2>&1 | sed -n 's/.*pdftocairo version \([0-9.]*\).*/\1/p' | head -n1)"
[ -n "$POPPLER_VERSION" ] || POPPLER_VERSION="unknown"

# --- Render + extract anchors -----------------------------------------------
mkdir -p "$OUTDIR"

# Deterministic page render at the pinned DPI -> page-1.png, page-2.png, ...
pdftocairo -png -r "$DPI" "$INPUT" "$OUTDIR/page"

# Anchors: metadata info dict + layout-preserving text.
pdfinfo "$INPUT" > "$OUTDIR/info.txt"
pdftotext -layout "$INPUT" "$OUTDIR/text.txt"

# --- Manifest ----------------------------------------------------------------
# Stable, numerically-sorted list of rendered page images. Built via command
# substitution so the accumulator survives (a piped `while` runs in a subshell).
pages_json="$(find "$OUTDIR" -maxdepth 1 -name 'page-*.png' | sort -t- -k2 -n | while IFS= read -r f; do
  printf '    "%s",\n' "$(basename "$f")"
done)"
pages_json="${pages_json%,}"

page_count="$(find "$OUTDIR" -maxdepth 1 -name 'page-*.png' | wc -l | tr -d ' ')"
src_base="$(basename "$INPUT")"

cat > "$OUTDIR/manifest.json" <<JSON
{
  "schema": "fugue.score-prep.v1",
  "source_pdf": "$src_base",
  "dpi": $DPI,
  "renderer": "poppler/pdftocairo",
  "poppler_version": "$POPPLER_VERSION",
  "page_count": $page_count,
  "pages": [
$pages_json
  ],
  "anchors": {
    "info": "info.txt",
    "text": "text.txt"
  },
  "produces": "fugue.score.v1"
}
JSON

printf 'prep_pdf: rendered %s page(s) at %s DPI (poppler %s) -> %s\n' \
  "$page_count" "$DPI" "$POPPLER_VERSION" "$OUTDIR" >&2
