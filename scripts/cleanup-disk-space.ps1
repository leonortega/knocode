#Requires -Version 5.1
<# ─────────────────────────────────────────────────────────────────────────────
 .SYNOPSIS
    Free disk space for this Rust/Cargo project (safe, dry-run by default).

 .DESCRIPTION
    Cleans project build artifacts, Cargo cache, Windows temp, recycle bin,
    and package manager caches. Shows sizes before deleting.

 .PARAMETER Apply
    Actually delete files. Without this flag, runs in dry-run (preview) mode.

 .EXAMPLE
    .\scripts\cleanup-disk-space.ps1           # dry-run (preview)
    .\scripts\cleanup-disk-space.ps1 -Apply    # actually delete
 ─────────────────────────────────────────────────────────────────────────────
#>

[CmdletBinding()]
param(
    [switch]$Apply
)

$ErrorActionPreference = "Continue"

# ── Helpers ──────────────────────────────────────────────────────────────────

function Get-DirSizeMB {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return 0 }
    try {
        $bytes = (Get-ChildItem -Path $Path -Recurse -File -ErrorAction SilentlyContinue |
                  Measure-Object -Property Length -Sum).Sum
        return [math]::Round($bytes / 1MB, 1)
    } catch {
        return 0
    }
}

function Remove-IfExists {
    param(
        [string]$Path,
        [string]$Label
    )
    if (-not (Test-Path $Path)) { return }

    $size = Get-DirSizeMB -Path $Path
    $script:freed += $size

    if (-not $Apply) {
        Write-Host "  [DRY-RUN] " -ForegroundColor Yellow -NoNewline
        Write-Host "would remove " -NoNewline
        Write-Host "$Path" -ForegroundColor Cyan -NoNewline
        Write-Host " (~$size MB)"
    } else {
        Write-Host "  Removing $Path (~$size MB)..." -ForegroundColor DarkYellow
        Remove-Item -Path $Path -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host "  [OK] Removed" -ForegroundColor Green
    }
}

# ── Init ─────────────────────────────────────────────────────────────────────

$script:freed = 0
$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot

$mode = if ($Apply) { "APPLY (deleting files!)" } else { "DRY-RUN (preview only)" }

Write-Host ""
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  Disk Space Cleanup" -ForegroundColor Cyan
Write-Host "  Project: $ProjectRoot" -ForegroundColor Cyan
Write-Host "  Mode:    $mode" -ForegroundColor $(if ($Apply) { "Red" } else { "Yellow" })
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""

# ═════════════════════════════════════════════════════════════════════════════
# 1. PROJECT BUILD ARTIFACTS
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "[1/6] Project build artifacts" -ForegroundColor Green
Remove-IfExists -Path "target"                                          -Label "Cargo target/"
Remove-IfExists -Path "experiments\ast-grep-tree-sitter-interop\target" -Label "Experiments target/"
Remove-IfExists -Path "node_modules"                                    -Label "Node.js dependencies"
Write-Host ""

# ═════════════════════════════════════════════════════════════════════════════
# 2. CARGO HOME CACHE
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "[2/6] Cargo home cache" -ForegroundColor Green
$CargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { "$env:USERPROFILE\.cargo" }
Remove-IfExists -Path "$CargoHome\registry\cache"   -Label "Registry cache (.crate files)"
Remove-IfExists -Path "$CargoHome\registry\src"     -Label "Registry source (extracted)"
Remove-IfExists -Path "$CargoHome\git\checkouts"    -Label "Git dependency checkouts"
Remove-IfExists -Path "$CargoHome\git\db"           -Label "Git dependency databases"
Remove-IfExists -Path "$CargoHome\archive"          -Label "Downloaded toolchain archives"
Write-Host "  Tip: 'cargo clean' removes only project target/, keeps cargo cache" -ForegroundColor DarkGray
Write-Host ""

# ═════════════════════════════════════════════════════════════════════════════
# 3. RUST TOOLCHAINS
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "[3/6] Rust toolchains" -ForegroundColor Green
if (Get-Command rustup -ErrorAction SilentlyContinue) {
    Write-Host "  Installed toolchains:" -ForegroundColor Cyan
    rustup toolchain list | ForEach-Object { Write-Host "    $_" }
    Write-Host ""
    Write-Host "  To remove old toolchains:  rustup toolchain uninstall <name>" -ForegroundColor DarkGray
    Write-Host "  To remove unused components: rustup component remove <name>" -ForegroundColor DarkGray
} else {
    Write-Host "  rustup not found, skipping" -ForegroundColor DarkGray
}
Write-Host ""

# ═════════════════════════════════════════════════════════════════════════════
# 4. WINDOWS TEMP FILES
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "[4/6] Windows temp files" -ForegroundColor Green
$TempDir = "$env:LOCALAPPDATA\Temp"
if (Test-Path $TempDir) {
    $tempSize = Get-DirSizeMB -Path $TempDir
    Write-Host "  Temp folder: $TempDir (~$tempSize MB)" -ForegroundColor Cyan

    if (-not $Apply) {
        Write-Host "  [DRY-RUN] Would clean temp files older than 7 days" -ForegroundColor Yellow
    } else {
        Write-Host "  Cleaning temp files older than 7 days..." -ForegroundColor DarkYellow
        $cutoff = (Get-Date).AddDays(-7)
        $cleaned = 0
        Get-ChildItem -Path $TempDir -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTime -lt $cutoff } |
            ForEach-Object {
                Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
                $cleaned++
            }
        Write-Host "  [OK] Cleaned $cleaned items" -ForegroundColor Green
        $script:freed += [math]::Round($tempSize * 0.3)  # rough estimate
    }
}
Write-Host ""

# ═════════════════════════════════════════════════════════════════════════════
# 5. RECYCLE BIN
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "[5/6] Recycle Bin" -ForegroundColor Green
if (-not $Apply) {
    Write-Host "  [DRY-RUN] Would empty recycle bin" -ForegroundColor Yellow
} else {
    Write-Host "  Emptying recycle bin..." -ForegroundColor DarkYellow
    try {
        Clear-RecycleBin -Force -ErrorAction Stop
        Write-Host "  [OK] Recycle bin emptied" -ForegroundColor Green
    } catch {
        Write-Host "  Recycle bin is already empty or access denied" -ForegroundColor DarkGray
    }
}
Write-Host ""

# ═════════════════════════════════════════════════════════════════════════════
# 6. PACKAGE MANAGER CACHES + MISC
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "[6/6] Package manager caches + misc" -ForegroundColor Green

# npm cache
$npmCache = "$env:LOCALAPPDATA\npm-cache"
Remove-IfExists -Path $npmCache                    -Label "npm cache"

# pnpm store
$pnpmStore = "$env:LOCALAPPDATA\pnpm-store"
Remove-IfExists -Path $pnpmStore                   -Label "pnpm store"

# yarn cache
try {
    $yarnCache = (yarn cache dir 2>$null)
    if ($yarnCache) { Remove-IfExists -Path $yarnCache -Label "yarn cache" }
} catch {}

# Project misc
Remove-IfExists -Path "eval\results"               -Label "Evaluation results"
Remove-IfExists -Path ".opencode\cache"            -Label "OpenCode cache"
Remove-IfExists -Path ".knocode\cache"             -Label "Knocode cache"
Write-Host ""

# ═════════════════════════════════════════════════════════════════════════════
# SUMMARY
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
if (-not $Apply) {
    Write-Host "  Estimated space to free: ~$([math]::Round($script:freed, 1)) MB" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  To actually delete, run:" -ForegroundColor White
    Write-Host "    .\scripts\cleanup-disk-space.ps1 -Apply" -ForegroundColor Cyan
} else {
    Write-Host "  Space freed: ~$([math]::Round($script:freed, 1)) MB" -ForegroundColor Green
}
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""
