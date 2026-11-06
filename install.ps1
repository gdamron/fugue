param(
  [string]$FugueBinDir = $(if ($env:FUGUE_BIN_DIR) { $env:FUGUE_BIN_DIR } else { Join-Path $HOME ".fugue\bin" }),
  [string]$FugueDocDir = $(if ($env:FUGUE_DOC_DIR) { $env:FUGUE_DOC_DIR } else { Join-Path $HOME ".fugue\share\doc" }),
  [string]$FugueVersion = $(if ($env:FUGUE_VERSION) { $env:FUGUE_VERSION } else { "latest" })
)

$ErrorActionPreference = "Stop"
$Repo = "gdamron/fugue"
$Target = "x86_64-pc-windows-msvc"
# The third-party license notices every release archive carries beside its binaries.
$NoticesFile = "THIRD_PARTY_LICENSES.txt"

function Write-Info([string]$Message) {
  Write-Host "==> $Message" -ForegroundColor Blue
}

function Get-AssetUrl([string]$Asset) {
  if ($FugueVersion -eq "latest") {
    return "https://github.com/$Repo/releases/latest/download/$Asset"
  }
  $Tag = $FugueVersion
  if (-not $Tag.StartsWith("v")) {
    $Tag = "v$Tag"
  }
  return "https://github.com/$Repo/releases/download/$Tag/$Asset"
}

# Downloads the combined install unit and extracts every Fugue executable from
# it. Installing them into the same directory lets fugue-mcp locate its sibling
# fugue daemon without requiring $FugueBinDir on PATH.
function Install-FugueUnit([string]$Asset, [string[]]$Binaries) {
  $Archive = Join-Path $Temp $Asset
  Write-Info "Downloading Fugue ($Asset)"
  Invoke-WebRequest -Uri (Get-AssetUrl $Asset) -OutFile $Archive
  tar -C $Temp -xzf $Archive
  foreach ($Binary in $Binaries) {
    $Extracted = Join-Path $Temp $Binary
    if (-not (Test-Path $Extracted)) {
      throw "archive $Asset did not contain expected binary '$Binary'"
    }
    Copy-Item $Extracted (Join-Path $FugueBinDir $Binary) -Force
    Write-Info "Installed $Binary -> $FugueBinDir"
  }
  Install-FugueNotices $Asset
}

# The binaries carry third-party code whose licenses require their notices to
# travel with them. A release without the file, or a doc directory that cannot
# be written, warns rather than failing: the binaries are already installed.
function Install-FugueNotices([string]$Asset) {
  $Extracted = Join-Path $Temp $NoticesFile
  if (-not (Test-Path $Extracted)) {
    Write-Warning "archive $Asset did not contain $NoticesFile; license notices were not installed"
    return
  }
  try {
    New-Item -ItemType Directory -Force -Path $FugueDocDir | Out-Null
    Copy-Item $Extracted (Join-Path $FugueDocDir $NoticesFile) -Force
    Write-Info "Installed license notices -> $(Join-Path $FugueDocDir $NoticesFile)"
  } catch {
    Write-Warning "could not install $NoticesFile into ${FugueDocDir}: $_"
  }
}

New-Item -ItemType Directory -Force -Path $FugueBinDir | Out-Null
$Temp = Join-Path ([System.IO.Path]::GetTempPath()) "fugue-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $Temp | Out-Null

try {
  Install-FugueUnit "fugue-tools-$Target.tar.gz" @("fugue.exe", "fugue-mcp.exe")
} finally {
  Remove-Item -Recurse -Force $Temp -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Info "Add $FugueBinDir to your PATH to use fugue and fugue-mcp."
Write-Host ""
Write-Info "Register the MCP server with:"
Write-Host ""
Write-Host "  claude mcp add fugue `"$FugueBinDir\fugue-mcp.exe`""
Write-Host ""
