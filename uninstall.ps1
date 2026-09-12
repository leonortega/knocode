# Knocode Windows uninstaller bootstrap (PowerShell).
#
# Downloads the full uninstaller (uninstall.ps1) from the latest GitHub
# Release into a temp file and executes it as a file.
#
#   irm https://raw.githubusercontent.com/leonortega/knocode/main/uninstall.ps1 | iex
#
# Non-interactive (download first, then run as a file):
#   irm https://raw.githubusercontent.com/leonortega/knocode/main/uninstall.ps1 -OutFile uninstall.ps1
#   powershell -ExecutionPolicy Bypass -File uninstall.ps1 -Force
#
# Parameters below are forwarded to the full uninstaller when run via -File.
# Env overrides (work via iex too): KNOCODE_UNINSTALL_FORCE=1 skips prompts.
param([switch]$KeepExternal, [switch]$KeepData, [switch]$KeepBuild, [switch]$Force, [switch]$DryRun, [switch]$RemoveRepo)

$ErrorActionPreference = "Stop"
$Repo = "leonortega/knocode"
if (-not $Force -and $env:KNOCODE_UNINSTALL_FORCE) { $Force = $true }

function Write-Step($m) { Write-Host "[knocode] $m" -ForegroundColor Cyan }
function Write-Warn2($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }

Write-Step "Knocode uninstaller bootstrap"

# 1. Resolve the uninstaller asset from the latest release.
$assetUrl = ""
try {
  $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "knocode-installer" } -UseBasicParsing
  $asset = $rel.assets | Where-Object { $_.name -eq "uninstall.ps1" } | Select-Object -First 1
  if (-not $asset) { throw "uninstall.ps1 not found in release $($rel.tag_name)" }
  $assetUrl = $asset.browser_download_url
  Write-Step "Latest release: $($rel.tag_name)"
}
catch {
  Write-Warn2 "release lookup failed: $($_.Exception.Message)"
  $assetUrl = "https://github.com/$Repo/releases/latest/download/uninstall.ps1"
}

# 2. Download to a temp file and verify it parses before running.
#    File execution (never piped expression) so a truncated download is a
#    clear error, not a block-comment parser failure.
try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 } catch { }
$uninstallFile = Join-Path $env:TEMP ("knocode-uninstall-" + [guid]::NewGuid().ToString("N") + ".ps1")
try {
  Invoke-WebRequest -Uri $assetUrl -OutFile $uninstallFile -UseBasicParsing
}
catch {
  Write-Fail "download failed ($assetUrl): $($_.Exception.Message)"
  exit 1
}
$raw = Get-Content -LiteralPath $uninstallFile -Raw
if (-not $raw -or $raw.Length -lt 500) {
  Remove-Item -LiteralPath $uninstallFile -Force -ErrorAction SilentlyContinue
  Write-Fail "downloaded uninstaller looks truncated - download manually from https://github.com/$Repo/releases"
  exit 1
}
$tokens = $null; $parseErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseInput($raw, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -gt 0) {
  Remove-Item -LiteralPath $uninstallFile -Force -ErrorAction SilentlyContinue
  Write-Fail "downloaded uninstaller failed to parse: $($parseErrors[0].Message)"
  exit 1
}

# 3. Run the verified uninstaller as a file, forwarding any parameters.
try {
  & $uninstallFile @PSBoundParameters
}
finally {
  Remove-Item -LiteralPath $uninstallFile -Force -ErrorAction SilentlyContinue
}
