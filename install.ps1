param(
  [string]$FugueBinDir = $(if ($env:FUGUE_BIN_DIR) { $env:FUGUE_BIN_DIR } else { Join-Path $HOME ".fugue\bin" }),
  [string]$FugueVersion = $(if ($env:FUGUE_VERSION) { $env:FUGUE_VERSION } else { "latest" })
)

$ErrorActionPreference = "Stop"
$Repo = "gdamron/fugue"
$Target = "x86_64-pc-windows-msvc"

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

function Install-FugueBinary([string]$Asset, [string]$Binary) {
  $Archive = Join-Path $Temp $Asset
  Write-Info "Downloading $Binary ($Asset)"
  Invoke-WebRequest -Uri (Get-AssetUrl $Asset) -OutFile $Archive
  tar -C $Temp -xzf $Archive
  $Extracted = Join-Path $Temp $Binary
  if (-not (Test-Path $Extracted)) {
    throw "archive $Asset did not contain expected binary '$Binary'"
  }
  Copy-Item $Extracted (Join-Path $FugueBinDir $Binary) -Force
  Write-Info "Installed $Binary -> $FugueBinDir"
}

New-Item -ItemType Directory -Force -Path $FugueBinDir | Out-Null
$Temp = Join-Path ([System.IO.Path]::GetTempPath()) "fugue-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $Temp | Out-Null

try {
  Install-FugueBinary "fugue-cli-$Target.tar.gz" "fugue.exe"
  Install-FugueBinary "fugue-mcp-$Target.tar.gz" "fugue-mcp.exe"
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
