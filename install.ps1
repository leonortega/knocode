# Knocode Windows installer bootstrap (PowerShell).
#
# Downloads the full installer (knocode-install.ps1) from the latest GitHub
# Release into a temp file and executes it as a file.
#
#   irm https://raw.githubusercontent.com/leonortega/knocode/main/install.ps1 | iex
#
# Pinned version (download first, then run as a file):
#   irm https://raw.githubusercontent.com/leonortega/knocode/main/install.ps1 -OutFile install.ps1
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Version <x.y.z>
#
# Parameters below are forwarded to the full installer when run via -File.
# Env overrides (codegraph-style, work via iex too): KNOCODE_VERSION, KNOCODE_AGENTS.
param([string]$Version = "", [string]$Agents = "", [switch]$AllAgents, [switch]$NoAgents, [switch]$WithRtk, [switch]$NoRtk, [switch]$SkipPrereqs)

$ErrorActionPreference = "Stop"
$Repo = "leonortega/knocode"
if (-not $Version -and $env:KNOCODE_VERSION) { $Version = $env:KNOCODE_VERSION }
if (-not $Agents -and $env:KNOCODE_AGENTS) { $Agents = $env:KNOCODE_AGENTS }

function Write-Step($m) { Write-Host "[knocode] $m" -ForegroundColor Cyan }
function Write-Warn2($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }

# 1. Resolve which release asset to run. -Version pins; default is latest.
$assetUrl = ""
if ($Version -ne "") {
  $ver = $Version.TrimStart("v")
  if ($ver -notmatch "^\d+\.\d+\.\d+") { Write-Fail "invalid -Version '$Version' (expected x.y.z)"; exit 1 }
  $assetUrl = "https://github.com/$Repo/releases/download/v$ver/knocode-install.ps1"
  Write-Step "Pinned version: $ver"
}
else {
  try {
    $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "knocode-installer" } -UseBasicParsing
    $asset = $rel.assets | Where-Object { $_.name -eq "knocode-install.ps1" } | Select-Object -First 1
    if (-not $asset) { throw "knocode-install.ps1 not found in release $($rel.tag_name)" }
    $assetUrl = $asset.browser_download_url
    Write-Step "Latest release: $($rel.tag_name)"
  }
  catch {
    Write-Warn2 "release lookup failed: $($_.Exception.Message)"
    $assetUrl = "https://github.com/$Repo/releases/latest/download/knocode-install.ps1"
  }
}

# 2. Download to a temp file and verify it parses before running.
#    File execution (never piped expression) so a truncated download is a
#    clear error, not a block-comment parser failure.
try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 } catch { }
$installerFile = Join-Path $env:TEMP ("knocode-install-" + [guid]::NewGuid().ToString("N") + ".ps1")
try {
  Invoke-WebRequest -Uri $assetUrl -OutFile $installerFile -UseBasicParsing
}
catch {
  Write-Fail "download failed ($assetUrl): $($_.Exception.Message)"
  exit 1
}
$raw = Get-Content -LiteralPath $installerFile -Raw
if (-not $raw -or $raw.Length -lt 500) {
  Remove-Item -LiteralPath $installerFile -Force -ErrorAction SilentlyContinue
  Write-Fail "downloaded installer looks truncated - download manually from https://github.com/$Repo/releases"
  exit 1
}
$tokens = $null; $parseErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseInput($raw, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -gt 0) {
  Remove-Item -LiteralPath $installerFile -Force -ErrorAction SilentlyContinue
  Write-Fail "downloaded installer failed to parse: $($parseErrors[0].Message)"
  exit 1
}

# 3. Run the verified installer as a file, forwarding any parameters.
try {
  & $installerFile @PSBoundParameters
}
finally {
  Remove-Item -LiteralPath $installerFile -Force -ErrorAction SilentlyContinue
}
