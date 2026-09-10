#Requires -Version 5.1
<#
.SYNOPSIS
  Knocode uninstaller (Windows PowerShell 5.1)
  Reverses scripts/install.ps1 - removes everything by default, preserves source unless -RemoveRepo.

.DESCRIPTION
  Default (no flags): stops daemon, removes project build artifacts (target/release/knocode*.exe),
  opencode plugins (project-local + global), RTK, and ALL user/project data
  (%USERPROFILE%\.knocode, .knocode/, sockets). Idempotent - safe to re-run.

  This is strict mode: no fallbacks. Default uninstalls everything. Use -KeepExternal / -KeepData
  to preserve tools or data. -KeepBuild preserves target/.

.PARAMETER KeepExternal
  Keep first-class external tools (do not uninstall rtk).

.PARAMETER KeepData
  Keep user and project data (do not delete %USERPROFILE%\.knocode or .knocode/).

.PARAMETER KeepBuild
  Keep target/ build artifacts (skip binary removal).

.PARAMETER RemoveExternal
  Legacy alias for default behavior (now default). Kept for backwards compat.

.PARAMETER RemoveData
  Legacy alias for default behavior (now default). Kept for backwards compat.

.PARAMETER Force
  Skip confirmation prompts (useful for CI). Without -Force, data removal prompts for confirmation.

.PARAMETER DryRun
  Preview what would be removed without deleting (alias for -WhatIf).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/uninstall.ps1
  # full uninstall: binaries + plugins + external tools + data (prompts for data)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/uninstall.ps1 -Force
  # full uninstall without prompt (CI)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/uninstall.ps1 -KeepData -KeepExternal
  # only binaries + plugins, keep tools and data (old default)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/uninstall.ps1 -DryRun
  # preview only

.NOTES
  Repository folders/files (.knocode/, target/, .opencode/) are NEVER deleted by default.
  Use -RemoveRepo to also delete repository artifacts (rarely needed).
#>
[CmdletBinding(SupportsShouldProcess=$true)]
param(
  [switch]$KeepExternal,
  [switch]$KeepData,
  [switch]$KeepBuild,
  [switch]$Force,
  [switch]$DryRun,
  [switch]$RemoveExternal,
  [switch]$RemoveData,
  [switch]$RemoveRepo
)

$ErrorActionPreference = "Continue"
# Always English in scripts
try { [System.Threading.Thread]::CurrentThread.CurrentUICulture = [System.Globalization.CultureInfo]::GetCultureInfo('en-US'); [System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::GetCultureInfo('en-US') } catch {}
$Root = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $Root

function Test-Cmd($cmd) { $null -ne (Get-Command $cmd -ErrorAction SilentlyContinue) }
function Info($m) { Write-Host "[knocode] $m" -ForegroundColor Cyan }
function Ok($m) { Write-Host "  [OK] $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkGray }

# Default is to remove external tools and global data, but NEVER repository folders unless -RemoveRepo
$doRemoveExternal = $RemoveExternal -or -not $KeepExternal
$doRemoveData = $RemoveData -or -not $KeepData
$doRemoveRepo = $RemoveRepo  # only if explicitly requested
# For non-interactive CI, skip prompt by default when -Force is not set but -WhatIf is not set either
# We will not prompt for global data removal - only for repository removal which is destructive to source

if ($DryRun) { $PSBoundParameters["WhatIf"] = $true; $WhatIfPreference = $true }

Info "Knocode uninstaller"
if ($WhatIfPreference) { Warn "DryRun/WhatIf active - no changes will be made" }
Info "Options: RemoveExternal( effective=$doRemoveExternal KeepExternal=$KeepExternal ) RemoveData( effective=$doRemoveData KeepData=$KeepData ) KeepBuild=$KeepBuild Force=$Force"

# 0. Confirmation for destructive data removal - only for repository data (global is safe to delete)
if ($doRemoveRepo -and $doRemoveData -and -not $Force -and -not $WhatIfPreference) {
  $msg = "This will permanently delete repository .knocode/ (config, index, database) at .knocode/. Global ~\.knocode will also be deleted. Continue?"
  $choice = Read-Host "$msg [y/N]"
  if ($choice -notin @("y","Y","yes","YES")) {
    Info "Aborted by user. Re-run with -Force to skip prompt or -KeepData to keep data."
    exit 0
  }
}

# 1. Stop daemon / clean socket
Info "Stopping daemon and cleaning socket..."

foreach ($procName in @("knocode-daemon","knocode")) {
  $procs = Get-Process -Name $procName -ErrorAction SilentlyContinue
  foreach ($p in $procs) {
    if ($PSCmdlet.ShouldProcess("Process $($p.ProcessName) PID $($p.Id)", "Stop-Process")) {
      try { Stop-Process -Id $p.Id -Force -ErrorAction Stop; Ok "stopped $procName PID $($p.Id)" } catch { Warn "failed to stop $procName PID $($p.Id): $_" }
    } else { Skip "would stop $procName PID $($p.Id)" }
  }
  if (-not $procs) { Skip "no running $procName process" }
}

$socketPaths = @(
  "$env:USERPROFILE\.knocode\knocode.sock",
  "/tmp/knocode.sock",
  (Join-Path $Root ".knocode/knocode.sock"),
  "/tmp/knocode.sock.lock"
)
$configToml = Join-Path $Root ".knocode/config.toml"
if (Test-Path $configToml) {
  try {
    $sockMatch = Select-String -Path $configToml -Pattern 'socket_path\s*=\s*"([^"]+)"' -ErrorAction SilentlyContinue
    if ($sockMatch) { $socketPaths += $sockMatch.Matches[0].Groups[1].Value }
  } catch {}
}
foreach ($sp in $socketPaths | Select-Object -Unique) {
  if (Test-Path $sp) {
    if ($PSCmdlet.ShouldProcess($sp, "Remove socket")) {
      try { Remove-Item -LiteralPath $sp -Force -ErrorAction Stop; Ok "removed socket $sp" } catch { Warn "failed to remove socket $sp : $_" }
    } else { Skip "would remove socket $sp" }
  }
}

# 1b. TASK-037: remove installed binaries (~\.knocode\bin) + revert USER PATH entry.
# Always executed: PATH is machine state, independent of -KeepData/-RemoveRepo.
Info "Removing installed knocode binaries from ~\.knocode\bin..."
$knocodeBinDir = Join-Path $env:USERPROFILE ".knocode\bin"
foreach ($bin in @("knocode.exe", "knocode-daemon.exe")) {
  $p = Join-Path $knocodeBinDir $bin
  if (Test-Path $p) {
    if ($PSCmdlet.ShouldProcess($p, "Remove-Item")) {
      try { Remove-Item -LiteralPath $p -Force -ErrorAction Stop; Ok "removed $p" } catch { Warn "failed to remove ${p}: $_" }
    } else { Skip "would remove $p" }
  } else { Skip "not found $p" }
}
if ((Test-Path $knocodeBinDir) -and -not (Get-ChildItem -LiteralPath $knocodeBinDir -Force -ErrorAction SilentlyContinue)) {
  if ($PSCmdlet.ShouldProcess($knocodeBinDir, "Remove empty dir")) {
    try { Remove-Item -LiteralPath $knocodeBinDir -Force -ErrorAction SilentlyContinue; Ok "removed empty $knocodeBinDir" } catch {}
  }
}
# Revert USER PATH (HKCU Environment) — only our exact entry, idempotent
try {
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if ($userPath) {
    $entries = $userPath -split ';' | Where-Object { $_ -ne '' }
    if ($entries -contains $knocodeBinDir) {
      $newUserPath = ($entries | Where-Object { $_ -ne $knocodeBinDir }) -join ';'
      if ($PSCmdlet.ShouldProcess("USER PATH", "Remove $knocodeBinDir")) {
        [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
        Ok "removed $knocodeBinDir from USER PATH"
      } else { Skip "would remove $knocodeBinDir from USER PATH" }
    } else { Skip "$knocodeBinDir not on USER PATH" }
    if (($env:Path -split ';') -contains $knocodeBinDir) { $env:Path = (($env:Path -split ';') | Where-Object { $_ -ne $knocodeBinDir }) -join ';' }
  }
} catch { Warn "could not revert USER PATH: $_" }

# 2. Remove build artifacts - repository folders are NEVER deleted by default (use -RemoveRepo)
if ($KeepBuild -or -not $doRemoveRepo) {
  Info "Skipping build artifact removal (repository folders preserved - use -RemoveRepo to delete target/)"
  Skip "keeping target/ (repository - not deleted)"
} else {
  Info "Removing build artifacts (--RemoveRepo)..."
  $binaries = @(
    "$Root\target\release\knocode.exe",
    "$Root\target\release\knocode-daemon.exe",
    "$Root\target\debug\knocode.exe",
    "$Root\target\debug\knocode-daemon.exe"
  )
  foreach ($bin in $binaries) {
    $display = $bin -replace [regex]::Escape($Root + "\"), "" -replace [regex]::Escape($Root + "/"), ""
    if (Test-Path $bin) {
      if ($PSCmdlet.ShouldProcess($display, "Remove-Item")) {
        try { Remove-Item -LiteralPath $bin -Force -ErrorAction Stop; Ok "removed $display" } catch { Warn "failed to remove $display : $_" }
      } else { Skip "would remove $display" }
    } else { Skip "not found $display" }
  }
  if (Test-Path "$Root\target") {
    if ($PSCmdlet.ShouldProcess("target/", "Remove-Item -Recurse")) {
      try { Remove-Item -LiteralPath "$Root\target" -Recurse -Force -ErrorAction Stop; Ok "removed target/ (cargo clean)" } catch { Warn "failed to remove target/: $_" }
    } else { Skip "would remove target/ (cargo clean)" }
  }
}

# 3. Remove opencode plugins - repository plugin is NEVER deleted by default (use -RemoveRepo) - use .opencode folder
Info "Removing opencode plugins..."
$pluginProject = Join-Path $Root ".opencode\plugins\knocode.ts"
$pluginGlobal = Join-Path $env:USERPROFILE ".config\opencode\plugins\knocode.ts"
# Global plugin (outside repo) - always delete - plugin 'knocode'
foreach ($g in @($pluginGlobal) | Select-Object -Unique) {
  if (Test-Path $g) {
    if ($PSCmdlet.ShouldProcess($g, "Remove-Item")) {
      try { Remove-Item -LiteralPath $g -Force -ErrorAction Stop; Ok "removed global plugin 'knocode'" } catch { Warn "failed to remove global plugin 'knocode': $_" }
    } else { Skip "would remove global plugin 'knocode'" }
  } else { Skip "not found global plugin 'knocode'" }
}
# Repository plugin - keep unless -RemoveRepo (use .opencode folder, plugin 'knocode')
if (Test-Path $pluginProject) {
  if ($doRemoveRepo) {
    if ($PSCmdlet.ShouldProcess(".opencode/plugins/knocode.ts", "Remove-Item")) {
      try { Remove-Item -LiteralPath $pluginProject -Force -ErrorAction Stop; Ok "removed plugin 'knocode' (--RemoveRepo)" } catch { Warn "failed to remove plugin 'knocode': $_" }
    } else { Skip "would remove plugin 'knocode'" }
  } else { Skip "keeping plugin 'knocode' (use -RemoveRepo to delete)" }
} else { Skip "not found plugin 'knocode'" }
# Only clean global empty dir by default; repo dir is kept
if ((Test-Path $env:USERPROFILE\.config\opencode\plugins) -and -not (Get-ChildItem -LiteralPath "$env:USERPROFILE\.config\opencode\plugins" -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne ".gitkeep" })) {
  if ($PSCmdlet.ShouldProcess("$env:USERPROFILE\.config\opencode\plugins", "Remove-Item")) {
    try { Remove-Item -LiteralPath "$env:USERPROFILE\.config\opencode\plugins" -Force -ErrorAction SilentlyContinue; Ok "removed empty global dir" } catch {}
  }
}
if ($doRemoveRepo -and (Test-Path "$Root\.opencode\plugins") -and -not (Get-ChildItem -LiteralPath "$Root\.opencode\plugins" -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne ".gitkeep" })) {
  if ($PSCmdlet.ShouldProcess(".opencode/plugins", "Remove-Item")) {
    try { Remove-Item -LiteralPath "$Root\.opencode\plugins" -Force -ErrorAction SilentlyContinue; Ok "removed empty .opencode/plugins (--RemoveRepo)" } catch {}
  }
}

# 3c. Knocode agent skill (opencode) - global ~\.config\opencode\skills\knocode (installed artifact, always)
$ocSkillGlobal = Join-Path $env:USERPROFILE ".config\opencode\skills\knocode"
if (Test-Path $ocSkillGlobal) {
  if ($PSCmdlet.ShouldProcess($ocSkillGlobal, "Remove-Item")) {
    try { Remove-Item -LiteralPath $ocSkillGlobal -Recurse -Force -ErrorAction Stop; Ok "removed global knocode skill" } catch { Warn "failed to remove global knocode skill: $_" }
  } else { Skip "would remove global knocode skill" }
} else { Skip "not found global knocode skill" }
# Remove empty global skills dir if only knocode was there
$ocSkillsDir = Join-Path $env:USERPROFILE ".config\opencode\skills"
if ((Test-Path $ocSkillsDir) -and -not (Get-ChildItem -LiteralPath $ocSkillsDir -Force -ErrorAction SilentlyContinue)) {
  if ($PSCmdlet.ShouldProcess($ocSkillsDir, "Remove-Item")) {
    try { Remove-Item -LiteralPath $ocSkillsDir -Force -ErrorAction SilentlyContinue; Ok "removed empty global skills dir" } catch {}
  }
}
# 3b. Remove MCP + plugin from opencode (always for knocode entries -- so plugin not showing after uninstall, file kept if has other config)
Info "Removing opencode plugin (opencode-knocode)..."
function Remove-OpencodeMcp($configPath, $isRepo) {
  $displayPath = $configPath
  if ($isRepo) { $displayPath = $configPath -replace [regex]::Escape($Root + "\"), "" -replace [regex]::Escape($Root + "/"), ""; if ($displayPath -eq $configPath) { $displayPath = Split-Path $configPath -Leaf } ; if ($configPath -match "\.opencode") { $displayPath = ".opencode/" + (Split-Path $configPath -Leaf) } else { $displayPath = $displayPath } }
  if (-not (Test-Path $configPath)) { Skip "MCP config not found at $displayPath"; return }
  # v1: always clean knocode plugin/mcp from .opencode, even without -RemoveRepo -- otherwise plugin/MCP still shows after uninstall+restart
  try {
    $raw = Get-Content -LiteralPath $configPath -Raw -ErrorAction SilentlyContinue
    if (-not $raw) { Skip "MCP config empty at $displayPath"; return }
    $noComments = $raw -replace '(?m)^\s*//.*$','' -replace '/\*.*?\*/',''
    $noComments = $noComments -replace ',\s*([\}\]])', '$1'
    $json = $null
    try {
      $obj = $noComments | ConvertFrom-Json -ErrorAction SilentlyContinue
      if ($obj) {
        $json = @{}
        foreach ($prop in $obj.PSObject.Properties) { $json[$prop.Name] = $prop.Value }
        if ($json.ContainsKey('mcp') -and $json['mcp'] -is [PSCustomObject]) {
          $mcpHash = @{}
          foreach ($p in $json['mcp'].PSObject.Properties) { $mcpHash[$p.Name] = $p.Value }
          $json['mcp'] = $mcpHash
        }
      }
    } catch {}
    if ($null -eq $json) { Skip "invalid JSON at $displayPath"; return }
    $removed = @(); $pluginRemoved = @()
    # handle plugin opencode-knocode / knocode
    if ($json.ContainsKey('plugin')) {
      $plugins = $json['plugin']
      $origCount = 0; $newPlugins = @()
      if ($plugins -is [System.Array]) { $origCount = $plugins.Count; $newPlugins = @($plugins | Where-Object { $_ -ne "opencode-knocode" -and $_ -ne "knocode" -and $_ -notlike "*opencode-knocode*" }) }
      elseif ($plugins -is [PSCustomObject]) { $origCount = 1; $newPlugins = @() }
      else { $origCount = 0; $newPlugins = @() }
      if ($origCount -ne $newPlugins.Count) {
        $pluginRemoved += "opencode-knocode"
        if ($newPlugins.Count -eq 0) { $json.Remove('plugin') } else { $json['plugin'] = $newPlugins }
        $removed += "plugin:opencode-knocode"
      }
    }
    # handle mcp (historical)
    if ($json.ContainsKey('mcp')) {
      $mcp = $json['mcp']
      if ($mcp -is [PSCustomObject]) {
        $tmp = @{}
        foreach ($p in $mcp.PSObject.Properties) { $tmp[$p.Name] = $p.Value }
        $mcp = $tmp; $json['mcp'] = $mcp
      }
      # historical MCP entries (no action)
      if ($mcp.Count -eq 0) { $json.Remove('mcp') }
    }
    if ($removed.Count -eq 0) { Skip "no knocode plugin/MCP entries at $displayPath"; return }
    # if file now only has $schema, remove it unless -RemoveRepo? keep file if has other keys, else clean
    $remainingKeys = @($json.Keys | Where-Object { $_ -ne '$schema' })
    if ($remainingKeys.Count -eq 0) {
      if ($PSCmdlet.ShouldProcess($configPath, "Remove empty config")) {
        try { Remove-Item -LiteralPath $configPath -Force -ErrorAction Stop; Ok "removed empty $displayPath (only knocode plugin/MCP)" } catch { Warn "failed to remove $displayPath : $_" }
      } else { Skip "would remove empty $displayPath" }
      return
    }
    if ($PSCmdlet.ShouldProcess($configPath, "Remove $removed")) {
      $out = $json | ConvertTo-Json -Depth 10
      [System.IO.File]::WriteAllText($configPath, $out, [System.Text.UTF8Encoding]::new($false))
      Ok "removed [$($removed -join ', ')] from $displayPath"
    } else { Skip "would remove [$($removed -join ', ')] from $displayPath" }
  } catch { Warn "MCP remove failed for $displayPath : $_" }
}
Remove-OpencodeMcp "$env:USERPROFILE\.config\opencode\opencode.jsonc" $false
Remove-OpencodeMcp "$env:USERPROFILE\.config\opencode\opencode.json" $false
Remove-OpencodeMcp (Join-Path $Root "opencode.jsonc") $true
Remove-OpencodeMcp (Join-Path $Root "opencode.json") $true
Remove-OpencodeMcp (Join-Path $Root ".opencode\opencode.jsonc") $true
Remove-OpencodeMcp (Join-Path $Root ".opencode\opencode.json") $true

# 4. Remove external tools (default: remove everything)
if (-not $doRemoveExternal) {
  Info "Skipping external tools (--KeepExternal)"
} else {
  Info "Removing external tools (strict default)..."

  # rtk: ~/.knocode/bin prebuilt (current) + legacy ~/bin + legacy cargo install
  foreach ($rtkPath in @("$env:USERPROFILE\.knocode\bin\rtk.exe", "$env:USERPROFILE\bin\rtk.exe")) {
    if (Test-Path $rtkPath) {
      if ($PSCmdlet.ShouldProcess($rtkPath, "Remove-Item")) {
        try { Remove-Item -LiteralPath $rtkPath -Force -ErrorAction Stop; Ok "removed rtk binary $rtkPath" } catch { Warn "failed to remove $rtkPath : $_" }
      } else { Skip "would remove rtk binary $rtkPath" }
    } else { Skip "not found rtk binary at $rtkPath" }
  }
  if (Test-Cmd rtk) {
    if ($PSCmdlet.ShouldProcess("rtk (legacy cargo)", "cargo uninstall rtk")) {
      try { cargo uninstall rtk 2>&1 | Out-Null; Ok "uninstalled rtk (legacy cargo)" } catch { Warn "rtk cargo uninstall failed: $_" }
    } else { Skip "would cargo uninstall rtk" }
  } else { Skip "no legacy cargo rtk on PATH" }



  # 3c. Opencode npm plugin -- installed into GLOBAL ~/.config/opencode/node_modules (and legacy .opencode/node_modules)
  Info "Removing opencode npm plugin (opencode-knocode)..."
  $ocGlobalDir = Join-Path $env:USERPROFILE ".config\opencode"
  $opencodeNodeModules = @(
    (Join-Path $ocGlobalDir "node_modules\opencode-knocode"),
    (Join-Path $ocGlobalDir "node_modules\@opencode-ai"),
    (Join-Path $ocGlobalDir "package-lock.json"),
    (Join-Path $Root ".opencode\node_modules\opencode-knocode"),
    (Join-Path $Root ".opencode\node_modules\@opencode-ai"),
    (Join-Path $Root ".opencode\package-lock.json"),
    (Join-Path $env:USERPROFILE ".cache\opencode\node_modules\opencode-knocode")
  )
  foreach ($p in $opencodeNodeModules) {
    $display = $p -replace [regex]::Escape($Root + "\"), "" -replace [regex]::Escape($Root + "/"), ""
    if ($display -eq $p) { $display = $p -replace [regex]::Escape($env:USERPROFILE + "\"), "~\" }
    if (Test-Path $p) {
      if ($PSCmdlet.ShouldProcess($display, "Remove-Item")) {
        try { Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop; Ok "removed $display (opencode npm plugin)" } catch { Warn "failed to remove $display : $_" }
      } else { Skip "would remove $display" }
    } else { Skip "not found $display" }
  }
  # Clean package.json deps: GLOBAL ~/.config/opencode/package.json + legacy .opencode/package.json
  if (-not $ocGlobalDir) { $ocGlobalDir = Join-Path $env:USERPROFILE ".config\opencode" }
  foreach ($pkgJsonPath in @((Join-Path $ocGlobalDir "package.json"), (Join-Path $Root ".opencode\package.json"))) {
    if (-not (Test-Path $pkgJsonPath)) { Skip "not found $pkgJsonPath"; continue }
    if ($PSCmdlet.ShouldProcess($pkgJsonPath, "clean opencode-knocode dep")) {
      try {
        $raw = Get-Content -LiteralPath $pkgJsonPath -Raw -ErrorAction Stop
        $obj = $raw | ConvertFrom-Json -ErrorAction Stop
        $changed = $false
        if ($obj.PSObject.Properties["dependencies"] -and $obj.dependencies.PSObject.Properties["opencode-knocode"]) {
          $obj.dependencies.PSObject.Properties.Remove("opencode-knocode"); $changed = $true
        }
        # If dependencies empty, remove file; else write back
        $depCount = 0; if ($obj.PSObject.Properties["dependencies"]) { $depCount = @($obj.dependencies.PSObject.Properties).Count }
        if ($depCount -eq 0) {
          Remove-Item -LiteralPath $pkgJsonPath -Force -ErrorAction Stop; Ok "removed empty $pkgJsonPath"
        } elseif ($changed) {
          $obj | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $pkgJsonPath -Encoding UTF8; Ok "cleaned opencode-knocode from $pkgJsonPath"
        } else { Skip "no opencode-knocode in $pkgJsonPath" }
      } catch { Warn "failed to clean ${pkgJsonPath} : $_" }
    } else { Skip "would clean $pkgJsonPath" }
  }
  # Remove empty .opencode dir if only empty after plugin removal (keep if has opencode.jsonc)
  if ((Test-Path "$Root\.opencode") -and -not (Get-ChildItem -LiteralPath "$Root\.opencode" -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -notin @(".gitkeep") })) {
    if ($PSCmdlet.ShouldProcess(".opencode", "Remove-Item")) {
      try { Remove-Item -LiteralPath "$Root\.opencode" -Recurse -Force -ErrorAction SilentlyContinue; Ok "removed empty .opencode/" } catch {}
    }
  } else { Skip "keeping .opencode/ (has opencode.jsonc or other config)" }
  # Global npm plugin (if ever published)
  if (Test-Cmd npm) {
    $hasGlobal = $false; try { npm list -g opencode-knocode 2>&1 | Out-Null; if ($LASTEXITCODE -eq 0) { $hasGlobal = $true } } catch {}
    if ($hasGlobal) {
      if ($PSCmdlet.ShouldProcess("opencode-knocode (npm -g)", "npm uninstall -g")) {
        try { npm uninstall -g opencode-knocode 2>&1 | Out-Null; Ok "uninstalled opencode-knocode (npm -g)" } catch { Warn "npm uninstall opencode-knocode failed: $_" }
      } else { Skip "would npm uninstall -g opencode-knocode" }
    } else { Skip "opencode-knocode not installed globally (npm -g)" }
  }
  # clippy (never uninstall rustup itself)
  if (Test-Cmd rustup) {
    if ($PSCmdlet.ShouldProcess("clippy (rustup component remove clippy)", "rustup component remove")) {
      try { rustup component remove clippy 2>&1 | Out-Null; Ok "removed rustup component clippy" } catch { Warn "failed to remove clippy: $_" }
    } else { Skip "would rustup component remove clippy" }
    Skip "keeping rustup toolchain (never uninstall rustup)"
  } else { Skip "rustup not installed" }
}

# 5. Remove data - global data is removed, repository .knocode is NEVER deleted by default (use -RemoveRepo) - use .opencode/.knocode relative
if (-not $doRemoveData) {
  Info "Skipping data removal (--KeepData)"
  Info "  Kept: $env:USERPROFILE\.knocode (global)"
  Info "  Kept: .knocode/ (repository - use -RemoveRepo to delete)"
} else {
  Info "Removing data (global only, repository preserved)..."
  $globalData = "$env:USERPROFILE\.knocode"
  if (Test-Path $globalData) {
    if ($PSCmdlet.ShouldProcess($globalData, "Remove-Item -Recurse")) {
      try { Remove-Item -LiteralPath $globalData -Recurse -Force -ErrorAction Stop; Ok "removed $globalData (DB, index, cache, logs, models)" } catch { Warn "failed to remove $globalData : $_" }
    } else { Skip "would remove $globalData" }
  } else { Skip "not found $globalData" }

  $projData = Join-Path $Root ".knocode"
  if (Test-Path $projData) {
    if ($doRemoveRepo) {
      if ($PSCmdlet.ShouldProcess($projData, "Remove-Item -Recurse")) {
        try { Remove-Item -LiteralPath $projData -Recurse -Force -ErrorAction Stop; Ok "removed .knocode/ (--RemoveRepo)" } catch { Warn "failed to remove .knocode/ : $_" }
      } else { Skip "would remove .knocode/" }
    } else { Skip "keeping repository .knocode/ (use -RemoveRepo to delete)" }
  } else { Skip "not found .knocode/" }
}

# 6. Final status
Info "Uninstall complete."
if (-not $doRemoveExternal) { Info "  External tools were kept (--KeepExternal)" } else { Info "  External tools removed" }
if (-not $doRemoveData) { Info "  Data was kept (--KeepData)" } else { Info "  Global data removed (repository .knocode preserved)" }
if ($KeepBuild -or -not $doRemoveRepo) { Info "  Build artifacts kept (repository preserved - use -RemoveRepo to delete)" } else { Info "  Build artifacts removed (--RemoveRepo)" }
Info "To reinstall: powershell -ExecutionPolicy Bypass -File scripts/install.ps1"
# Only warn if global plugin still present after uninstall (repo plugin is intentionally kept)
if (Test-Path "$env:USERPROFILE\.config\opencode\plugins\knocode.ts") { Warn "global plugin still present at $env:USERPROFILE\.config\opencode\plugins\knocode.ts - may need manual removal or restart opencode" }
if ($doRemoveRepo -and (Test-Path "$Root\.opencode\plugins\knocode.ts")) { Warn "repository plugin still present at .opencode/plugins/knocode.ts even after --RemoveRepo" }
if ($doRemoveExternal) {
  if (Test-Path "$env:USERPROFILE\.config\opencode\package.json") { Warn "global ~\.config\opencode\package.json still references knocode deps - inspect before removing (may contain your own deps)" }
}
