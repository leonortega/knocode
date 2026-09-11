#Requires -Version 5.1
<#
.SYNOPSIS
  Knocode Windows installer bootstrap.

.DESCRIPTION
  This is a thin bootstrapper hosted on GitHub Pages.
  It downloads the full installer from the latest GitHub Release and executes it.

  One-liner:
    powershell -ExecutionPolicy Bypass -c "irm https://leonortega.github.io/knocode/install.ps1 | iex"
#>

$ErrorActionPreference = "Stop"
$Repo = "leonortega/knocode"

# Resolve the latest release and grab the full installer
try {
    $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "knocode-installer" }
    $installer = $rel.assets | Where-Object { $_.name -eq "knocode-install.ps1" } | Select-Object -First 1
    if (-not $installer) {
        throw "knocode-install.ps1 not found in latest release $($rel.tag_name)"
    }
    Write-Host "[knocode] Downloading installer from $($rel.tag_name)..." -ForegroundColor Cyan
    $tmpFile = Join-Path $env:TEMP "knocode-install-$(Get-Random).ps1"
    Invoke-WebRequest -Uri $installer.browser_download_url -OutFile $tmpFile -UseBasicParsing
    & $tmpFile @PSBoundParameters
    Remove-Item $tmpFile -ErrorAction SilentlyContinue
}
catch {
    Write-Host "[knocode] Bootstrap failed: $_" -ForegroundColor Red
    Write-Host "[knocode] Falling back to direct download..." -ForegroundColor Yellow
    irm "https://github.com/$Repo/releases/latest/download/knocode-install.ps1" | iex
}
