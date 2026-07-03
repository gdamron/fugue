<#
.SYNOPSIS
  Deterministic PDF preprocessing for the import-score-from-pdf skill (Windows).

.DESCRIPTION
  PowerShell equivalent of prep_pdf.sh for native Windows. Renders a score PDF's
  pages to PNGs at a fixed DPI and extracts text/metadata anchors using poppler,
  then writes a manifest.json. Behavior and output are identical to prep_pdf.sh so
  the skill is agent-agnostic across platforms.

  We deliberately do NOT rasterize PDFs ourselves — poppler is the renderer the
  surrounding toolchain already assumes, and pinning it (version recorded in the
  manifest) is how we get determinism. See the decision ledger entry
  .harness/decisions/2026-06-30-judgment.md.

.PARAMETER Pdf
  Path to the input PDF.

.PARAMETER OutDir
  Output directory (created if missing).

.PARAMETER Dpi
  Render resolution in DPI (default: 200). Pinned for reproducibility.

.PARAMETER Install
  If poppler is missing, run the platform install command instead of only printing
  it. (Install mutates your system — see preflight.)

.EXAMPLE
  ./prep_pdf.ps1 -Install path\to\score.pdf out\
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true, Position = 0)] [string] $Pdf,
  [Parameter(Mandatory = $true, Position = 1)] [string] $OutDir,
  [int] $Dpi = 200,
  [switch] $Install
)

$ErrorActionPreference = 'Stop'

function Die($msg) { Write-Error "prep_pdf: $msg"; exit 1 }

if (-not (Test-Path -LiteralPath $Pdf -PathType Leaf)) { Die "input PDF not found: $Pdf" }
if ($Dpi -le 0) { Die "-Dpi must be a positive integer" }

# --- Preflight: ensure poppler (pdftocairo, pdftotext, pdfinfo) --------------
function Get-InstallCmd {
  if (Get-Command winget -ErrorAction SilentlyContinue) { return "winget install --id oschwartz10612.Poppler -e" }
  elseif (Get-Command choco -ErrorAction SilentlyContinue) { return "choco install -y poppler" }
  elseif (Get-Command scoop -ErrorAction SilentlyContinue) { return "scoop install poppler" }
  else { return "" }
}

$missing = @('pdftocairo', 'pdftotext', 'pdfinfo') | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }
if ($missing) {
  $cmd = Get-InstallCmd
  if (-not $cmd) { Die "poppler is required (missing: $($missing -join ' ')) but no supported package manager (winget/choco/scoop) was found. Install poppler manually and ensure its bin\ is on PATH." }
  if ($Install) {
    Write-Host "prep_pdf: poppler missing ($($missing -join ' ')) — installing with:`n  $cmd"
    Invoke-Expression $cmd
    if ($LASTEXITCODE -ne 0) { Die "poppler install failed; run it manually: $cmd" }
  } else {
    Die "poppler is required (missing: $($missing -join ' ')). Install it, then re-run:`n  $cmd`nor re-run this script with -Install to do it for you."
  }
}

$popplerVersion = 'unknown'
$verOut = (& pdftocairo -v 2>&1) -join "`n"
if ($verOut -match 'pdftocairo version ([0-9.]+)') { $popplerVersion = $Matches[1] }

# --- Render + extract anchors -----------------------------------------------
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# Deterministic page render at the pinned DPI -> page-1.png, page-2.png, ...
& pdftocairo -png -r $Dpi $Pdf (Join-Path $OutDir 'page')

# Anchors: metadata info dict + layout-preserving text.
& pdfinfo $Pdf | Out-File -FilePath (Join-Path $OutDir 'info.txt') -Encoding utf8
& pdftotext -layout $Pdf (Join-Path $OutDir 'text.txt')

# --- Manifest ----------------------------------------------------------------
# Numerically-sorted list of rendered page images (page-2 before page-10).
$pages = Get-ChildItem -LiteralPath $OutDir -Filter 'page-*.png' |
  Sort-Object { [int]($_.BaseName -replace '^page-', '') } |
  ForEach-Object { $_.Name }

$manifest = [ordered]@{
  schema          = 'fugue.score-prep.v1'
  source_pdf      = [System.IO.Path]::GetFileName($Pdf)
  dpi             = $Dpi
  renderer        = 'poppler/pdftocairo'
  poppler_version = $popplerVersion
  page_count      = @($pages).Count
  pages           = @($pages)
  anchors         = [ordered]@{ info = 'info.txt'; text = 'text.txt' }
  produces        = 'fugue.score.v1'
}
$manifest | ConvertTo-Json -Depth 5 | Out-File -FilePath (Join-Path $OutDir 'manifest.json') -Encoding utf8

Write-Host "prep_pdf: rendered $(@($pages).Count) page(s) at $Dpi DPI (poppler $popplerVersion) -> $OutDir"
