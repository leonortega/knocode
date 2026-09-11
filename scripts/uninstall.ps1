#Requires -Version 5.1
<#
.SYNOPSIS
  Knocode uninstaller (Windows PowerShell 5.1)
  Reverses scripts/install.ps1 - removes everything by default, preserves source unless -RemoveRepo.
  Also runs standalone (download uninstall.ps1 from the latest GitHub Release) -
  repository steps are skipped when no source checkout is present.

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
# UTF-8 WITHOUT BOM: PowerShell 5.1 `Set-Content -Encoding UTF8` writes a BOM,
# which breaks strict JSON/TOML parsers (seen: Cline rejecting MCP configs).
function Set-Utf8NoBom($path, $content) { [IO.File]::WriteAllText($path, [string]$content, (New-Object System.Text.UTF8Encoding($false))) }

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
# RTK opencode file plugin (created by `rtk init -g --opencode`, no documented --uninstall).
# Global copy is an installed artifact - always delete. Repo copy only with -RemoveRepo.
$rtkPluginGlobal = Join-Path $env:USERPROFILE ".config\opencode\plugins\rtk.ts"
$rtkPluginProject = Join-Path $Root ".opencode\plugins\rtk.ts"
if (Test-Path $rtkPluginGlobal) {
  if ($PSCmdlet.ShouldProcess($rtkPluginGlobal, "Remove-Item")) {
    try { Remove-Item -LiteralPath $rtkPluginGlobal -Force -ErrorAction Stop; Ok "removed global plugin 'rtk' (rtk init artifact)" } catch { Warn "failed to remove global plugin 'rtk': $_" }
  } else { Skip "would remove global plugin 'rtk'" }
} else { Skip "not found global plugin 'rtk'" }
if (Test-Path $rtkPluginProject) {
  if ($doRemoveRepo) {
    if ($PSCmdlet.ShouldProcess(".opencode/plugins/rtk.ts", "Remove-Item")) {
      try { Remove-Item -LiteralPath $rtkPluginProject -Force -ErrorAction Stop; Ok "removed plugin 'rtk' (--RemoveRepo)" } catch { Warn "failed to remove plugin 'rtk': $_" }
    } else { Skip "would remove plugin 'rtk'" }
  } else { Skip "keeping plugin 'rtk' (use -RemoveRepo to delete)" }
} else { Skip "not found plugin 'rtk'" }
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
# 3d. Knocode Copilot artifacts (created by all installers when copilot is wired).
# Hooks file + skill are installed artifacts - always delete. The Agent Plugin dir
# (~\.knocode\copilot-plugin) lives under global data and is removed with it.
Info "Removing Copilot hooks and skill (knocode)..."
$cpHooksFile = Join-Path $env:USERPROFILE ".copilot\hooks\knocode-context.json"
if (Test-Path $cpHooksFile) {
  if ($PSCmdlet.ShouldProcess($cpHooksFile, "Remove-Item")) {
    try { Remove-Item -LiteralPath $cpHooksFile -Force -ErrorAction Stop; Ok "removed Copilot hooks (knocode-context.json)" } catch { Warn "failed to remove Copilot hooks: $_" }
  } else { Skip "would remove Copilot hooks (knocode-context.json)" }
} else { Skip "not found Copilot hooks (knocode-context.json)" }
$cpHooksDir = Join-Path $env:USERPROFILE ".copilot\hooks"
if ((Test-Path $cpHooksDir) -and -not (Get-ChildItem -LiteralPath $cpHooksDir -Force -ErrorAction SilentlyContinue)) {
  if ($PSCmdlet.ShouldProcess($cpHooksDir, "Remove-Item")) {
    try { Remove-Item -LiteralPath $cpHooksDir -Force -ErrorAction SilentlyContinue; Ok "removed empty Copilot hooks dir" } catch {}
  }
}
$cpSkillGlobal = Join-Path $env:USERPROFILE ".copilot\skills\knocode"
if (Test-Path $cpSkillGlobal) {
  if ($PSCmdlet.ShouldProcess($cpSkillGlobal, "Remove-Item")) {
    try { Remove-Item -LiteralPath $cpSkillGlobal -Recurse -Force -ErrorAction Stop; Ok "removed Copilot knocode skill" } catch { Warn "failed to remove Copilot knocode skill: $_" }
  } else { Skip "would remove Copilot knocode skill" }
} else { Skip "not found Copilot knocode skill" }
$cpSkillsDir = Join-Path $env:USERPROFILE ".copilot\skills"
if ((Test-Path $cpSkillsDir) -and -not (Get-ChildItem -LiteralPath $cpSkillsDir -Force -ErrorAction SilentlyContinue)) {
  if ($PSCmdlet.ShouldProcess($cpSkillsDir, "Remove-Item")) {
    try { Remove-Item -LiteralPath $cpSkillsDir -Force -ErrorAction SilentlyContinue; Ok "removed empty Copilot skills dir" } catch {}
  }
}
# 3g. Universal agents (claude/cursor/gemini/codex/cline): skill dirs + `knocode`
# MCP entries. Config files are NEVER deleted — only the knocode keys are
# removed (claude.json holds projects/history; settings files hold user prefs).
Info "Removing universal agent wiring (knocode)..."
function Remove-UniversalSkill($agent, $skillDir, $displayPath) {
  if (Test-Path $skillDir) {
    if ($PSCmdlet.ShouldProcess($displayPath, "Remove-Item")) {
      try { Remove-Item -LiteralPath $skillDir -Recurse -Force -ErrorAction Stop; Ok "removed $agent skill ($displayPath)" } catch { Warn "failed to remove $agent skill: $_" }
    } else { Skip "would remove $agent skill ($displayPath)" }
  } else { Skip "not found $agent skill ($displayPath)" }
  $parent = Split-Path $skillDir
  if ((Test-Path $parent) -and -not (Get-ChildItem -LiteralPath $parent -Force -ErrorAction SilentlyContinue)) {
    if ($PSCmdlet.ShouldProcess($parent, "Remove-Item")) {
      try { Remove-Item -LiteralPath $parent -Force -ErrorAction SilentlyContinue; Ok "removed empty $agent skills dir" } catch {}
    }
  }
}
function Remove-KnocodeMcpJson($agent, $configPath, $displayPath) {
  if (-not (Test-Path $configPath)) { Skip "MCP config not found at $displayPath"; return }
  try {
    $raw = Get-Content -LiteralPath $configPath -Raw -ErrorAction Stop
    if (-not $raw -or -not $raw.Trim()) { Skip "MCP config empty at $displayPath"; return }
    $obj = $null
    try { $obj = $raw | ConvertFrom-Json -ErrorAction Stop }
    catch {
      $clean = ($raw -replace '(?m)^\s*//.*$','' -replace '/\*.*?\*/','') -replace ',\s*([\}\]])', '$1'
      $obj = $clean | ConvertFrom-Json -ErrorAction Stop
    }
    if (-not $obj.PSObject.Properties['mcpServers']) { Skip "no mcpServers at $displayPath"; return }
    $servers = $obj.mcpServers
    $has = $false
    if ($servers -is [PSCustomObject] -and $servers.PSObject.Properties['knocode']) { $has = $true }
    elseif ($servers -is [System.Collections.IDictionary] -and $servers.Contains('knocode')) { $has = $true }
    if (-not $has) { Skip "no knocode MCP entry at $displayPath"; return }
    if ($PSCmdlet.ShouldProcess($displayPath, "Remove mcpServers.knocode")) {
      if ($servers -is [PSCustomObject]) { $servers.PSObject.Properties.Remove('knocode') } else { $servers.Remove('knocode') }
      $remaining = @()
      if ($servers -is [PSCustomObject]) { $remaining = @($servers.PSObject.Properties) } elseif ($servers.Count -gt 0) { $remaining = @($true) }
      if ($remaining.Count -eq 0) { $obj.PSObject.Properties.Remove('mcpServers') }
      Set-Utf8NoBom $configPath ($obj | ConvertTo-Json -Depth 10)
      Ok "removed knocode MCP entry from $displayPath"
    } else { Skip "would remove knocode MCP entry from $displayPath" }
  } catch { Warn "MCP remove failed for $displayPath : $_" }
}
Remove-UniversalSkill "claude" (Join-Path $env:USERPROFILE ".claude\skills\knocode") "~\.claude\skills\knocode"
Remove-KnocodeMcpJson "claude" (Join-Path $env:USERPROFILE ".claude.json") "~\.claude.json"
Remove-UniversalSkill "cursor" (Join-Path $env:USERPROFILE ".cursor\skills\knocode") "~\.cursor\skills\knocode"
Remove-KnocodeMcpJson "cursor" (Join-Path $env:USERPROFILE ".cursor\mcp.json") "~\.cursor\mcp.json"
Remove-UniversalSkill "gemini" (Join-Path $env:USERPROFILE ".gemini\skills\knocode") "~\.gemini\skills\knocode"
Remove-KnocodeMcpJson "gemini" (Join-Path $env:USERPROFILE ".gemini\settings.json") "~\.gemini\settings.json"
Remove-UniversalSkill "cline" (Join-Path $env:USERPROFILE ".cline\skills\knocode") "~\.cline\skills\knocode"
Remove-KnocodeMcpJson "cline" (Join-Path $env:USERPROFILE ".cline\data\settings\cline_mcp_settings.json") "~\.cline\data\settings\cline_mcp_settings.json"
# Codex: skill lives under ~/.knocode/skills + TOML entries in ~/.codex/config.toml
Remove-UniversalSkill "codex" (Join-Path $env:USERPROFILE ".knocode\skills\knocode") "~\.knocode\skills\knocode"
function Remove-CodexKnocodeEntries($codexCfg) {
  if (-not (Test-Path $codexCfg)) { Skip "MCP config not found at ~\.codex\config.toml"; return }
  try {
    $lines = @(Get-Content -LiteralPath $codexCfg -ErrorAction Stop)
    # Block-aware removal: drop [mcp_servers.knocode] and only those
    # [[skills.config]] blocks that reference knocode; keep preamble + rest.
    $bounds = @()
    for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^\s*\[') { $bounds += $i } }
    $out = @()
    if ($bounds.Count -gt 0 -and $bounds[0] -gt 0) { $out += $lines[0..($bounds[0] - 1)] }
    $bounds += $lines.Count
    $changed = $false
    for ($b = 0; $b -lt ($bounds.Count - 1); $b++) {
      $block = @($lines[$bounds[$b]..($bounds[$b + 1] - 1)])
      $head = $lines[$bounds[$b]]
      $drop = $false
      if ($head -match '^\s*\[mcp_servers\.knocode\]') { $drop = $true }
      elseif ($head -match '^\s*\[\[skills\.config\]\]') { $drop = (($block -join "`n") -match 'knocode') }
      if ($drop) { $changed = $true } else { $out += $block }
    }
    if (-not $changed) { Skip "no knocode entries in ~\.codex\config.toml" }
    elseif ($PSCmdlet.ShouldProcess("~\.codex\config.toml", "Remove knocode entries")) {
      Set-Utf8NoBom $codexCfg ($out -join "`r`n")
      Ok "removed knocode entries from ~\.codex\config.toml"
    } else { Skip "would remove knocode entries from ~\.codex\config.toml" }
  } catch { Warn "codex config cleanup failed: $_" }
}
Remove-CodexKnocodeEntries (Join-Path $env:USERPROFILE ".codex\config.toml")
# Shared MCP bridge (installed artifact like binaries - always remove).
$bridgeDir = Join-Path $env:USERPROFILE ".knocode\mcp-server"
if (Test-Path $bridgeDir) {
  if ($PSCmdlet.ShouldProcess($bridgeDir, "Remove-Item")) {
    try { Remove-Item -LiteralPath $bridgeDir -Recurse -Force -ErrorAction Stop; Ok "removed shared MCP bridge ($bridgeDir)" } catch { Warn "failed to remove MCP bridge: $_" }
  } else { Skip "would remove shared MCP bridge ($bridgeDir)" }
} else { Skip "not found shared MCP bridge ($bridgeDir)" }
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
    # handle plugin opencode-knocode / knocode / rtk (rtk file plugin has no documented --uninstall)
    if ($json.ContainsKey('plugin')) {
      $plugins = $json['plugin']
      $origCount = 0; $newPlugins = @()
      if ($plugins -is [System.Array]) { $origCount = $plugins.Count; $newPlugins = @($plugins | Where-Object { $_ -ne "opencode-knocode" -and $_ -ne "knocode" -and $_ -ne "rtk" -and $_ -notlike "*opencode-knocode*" }) }
      elseif ($plugins -is [PSCustomObject]) { $origCount = 1; $newPlugins = @() }
      else { $origCount = 0; $newPlugins = @() }
      if ($origCount -ne $newPlugins.Count) {
        $pluginRemoved += "opencode-knocode"
        if ($newPlugins.Count -eq 0) { $json.Remove('plugin') } else { $json['plugin'] = $newPlugins }
        $removed += "plugin:opencode-knocode,rtk"
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

# 3e. VS Code extension (installed by scripts/install.ps1 via `code --install-extension`)
Info "Removing VS Code extension (knocode)..."
if (Test-Cmd code) {
  if ($PSCmdlet.ShouldProcess("knocode.knocode-copilot-extension", "code --uninstall-extension")) {
    try { & code --uninstall-extension knocode.knocode-copilot-extension 2>&1 | Out-Null; Ok "uninstalled VS Code extension knocode.knocode-copilot-extension" } catch { Warn "VS Code extension uninstall failed: $_" }
  } else { Skip "would uninstall VS Code extension knocode.knocode-copilot-extension" }
} else { Skip "VS Code CLI (code) not on PATH" }
# Fallback: extension dir stays behind when `code` is missing or the uninstall
# errors — remove it directly so the @knocode participant can't survive uninstall.
foreach ($extParent in @((Join-Path $env:USERPROFILE ".vscode\extensions"))) {
  foreach ($extDir in @(Get-ChildItem -LiteralPath $extParent -Directory -Filter "knocode.knocode-copilot-extension-*" -ErrorAction SilentlyContinue)) {
    if ($PSCmdlet.ShouldProcess($extDir.FullName, "Remove-Item")) {
      try { Remove-Item -LiteralPath $extDir.FullName -Recurse -Force -ErrorAction Stop; Ok "removed VS Code extension dir $($extDir.Name)" } catch { Warn "failed to remove VS Code extension dir $($extDir.Name): $_" }
    } else { Skip "would remove VS Code extension dir $($extDir.Name)" }
  }
}

# 3f. VS Code Agent Plugin registration (settings.json) + prompt cache.
# The plugin is registered manually (chat.pluginLocations) or copied into the
# agentPlugins cache — uninstall removes the source dir with global data, but
# the settings entries survive and keep VS Code loading stale hooks/skill.
Info "Removing VS Code Agent Plugin registration (knocode)..."
function Remove-VscodeKnocodeRegistration($settingsPath, $displayPath) {
  if (-not (Test-Path $settingsPath)) { Skip "settings not found at $displayPath"; return }
  try {
    $raw = Get-Content -LiteralPath $settingsPath -Raw -ErrorAction Stop
    if (-not $raw -or -not $raw.Trim()) { Skip "settings empty at $displayPath"; return }
    $noComments = $raw -replace '(?m)^\s*//.*$','' -replace '/\*.*?\*/',''
    $noComments = $noComments -replace ',\s*([\}\]])', '$1'
    $obj = $noComments | ConvertFrom-Json -ErrorAction Stop
    $changed = $false
    if ($obj.PSObject.Properties['chat.pluginLocations'] -and $obj.'chat.pluginLocations' -is [PSCustomObject]) {
      foreach ($prop in @($obj.'chat.pluginLocations'.PSObject.Properties)) {
        if ($prop.Name -match 'knocode|copilot-plugin') {
          $obj.'chat.pluginLocations'.PSObject.Properties.Remove($prop.Name); $changed = $true
        }
      }
      if (-not @($obj.'chat.pluginLocations'.PSObject.Properties)) {
        $obj.PSObject.Properties.Remove('chat.pluginLocations'); $changed = $true
      }
    }
    if ($obj.PSObject.Properties['enabledPlugins'] -and $obj.'enabledPlugins' -is [PSCustomObject]) {
      foreach ($prop in @($obj.'enabledPlugins'.PSObject.Properties)) {
        if ($prop.Name -match 'knocode') {
          $obj.'enabledPlugins'.PSObject.Properties.Remove($prop.Name); $changed = $true
        }
      }
      if (-not @($obj.'enabledPlugins'.PSObject.Properties)) {
        $obj.PSObject.Properties.Remove('enabledPlugins'); $changed = $true
      }
    }
    if (-not $changed) { Skip "no knocode plugin entries at $displayPath"; return }
    if ($PSCmdlet.ShouldProcess($displayPath, "Remove knocode plugin registration")) {
      Set-Utf8NoBom $settingsPath ($obj | ConvertTo-Json -Depth 10)
      Ok "removed knocode plugin registration from $displayPath"
    } else { Skip "would remove knocode plugin registration from $displayPath" }
  } catch { Warn "settings cleanup failed for $displayPath : $_" }
}
Remove-VscodeKnocodeRegistration (Join-Path $env:APPDATA "Code\User\settings.json") "%APPDATA%\Code\User\settings.json"
Remove-VscodeKnocodeRegistration (Join-Path $Root ".vscode\settings.json") ".vscode/settings.json"
# Prompt-cache sidecars (UserPromptSubmit -> PreToolUse handoff). Stale files would
# inject one outdated context block after a reinstall — always safe to delete.
foreach ($cacheDir in @((Join-Path ([IO.Path]::GetTempPath()) "knocode-hooks"))) {
  if (Test-Path $cacheDir) {
    if ($PSCmdlet.ShouldProcess($cacheDir, "Remove-Item")) {
      try { Remove-Item -LiteralPath $cacheDir -Recurse -Force -ErrorAction Stop; Ok "removed prompt cache $cacheDir" } catch { Warn "failed to remove prompt cache $cacheDir : $_" }
    } else { Skip "would remove prompt cache $cacheDir" }
  } else { Skip "not found prompt cache $cacheDir" }
}
if ($env:PLUGIN_DATA) {
  $pluginCache = Join-Path $env:PLUGIN_DATA "knocode-hooks"
  if (Test-Path $pluginCache) {
    if ($PSCmdlet.ShouldProcess($pluginCache, "Remove-Item")) {
      try { Remove-Item -LiteralPath $pluginCache -Recurse -Force -ErrorAction Stop; Ok "removed prompt cache $pluginCache" } catch { Warn "failed to remove prompt cache $pluginCache : $_" }
    } else { Skip "would remove prompt cache $pluginCache" }
  } else { Skip "not found prompt cache $pluginCache" }
}

# 4. Remove external tools (default: remove everything)
if (-not $doRemoveExternal) {
  Info "Skipping external tools (--KeepExternal)"
} else {
  Info "Removing external tools (strict default)..."

  # rtk integrations: official uninstall FIRST (fail-open, idempotent), binary delete after.
  # Mirrors the installer's per-agent map (global hook + opencode/copilot/cursor/
  # gemini/codex). Cursor/gemini/codex --uninstall shapes are symmetric guesses —
  # rtk rejects unknown combos with an error, which stays a Warn, never fatal.
  # `rtk init -g --uninstall` covers the global hook only; opencode is file-based
  # (~/.config/opencode/plugins/rtk.ts, removed in section 3) with no documented --uninstall.
  # Cline is project-scoped .clinerules (never written by the installer) — nothing to remove.
  $rtkExe = $null
  foreach ($cand in @("$env:USERPROFILE\.knocode\bin\rtk.exe", "$env:USERPROFILE\bin\rtk.exe")) {
    if (Test-Path $cand) { $rtkExe = $cand; break }
  }
  if (-not $rtkExe -and (Test-Cmd rtk)) {
    try { $rtkExe = (Get-Command rtk -ErrorAction SilentlyContinue).Source } catch {}
  }
  if ($rtkExe) {
    $rtkUninstalls = @(
      @("init", "-g", "--uninstall"),
      @("init", "-g", "--opencode", "--uninstall"),
      @("init", "--uninstall", "--global", "--copilot"),
      @("init", "--uninstall", "--copilot"),
      @("init", "-g", "--agent", "cursor", "--uninstall"),
      @("init", "-g", "--gemini", "--uninstall"),
      @("init", "-g", "--codex", "--uninstall")
    )
    foreach ($rtkArgs in $rtkUninstalls) {
      $display = "rtk $($rtkArgs -join ' ')"
      if ($PSCmdlet.ShouldProcess($display, "run rtk uninstall")) {
        try { & $rtkExe @rtkArgs 2>&1 | Out-Null; Ok "ran $display" } catch { Warn "$display failed: $_" }
      } else { Skip "would run $display" }
    }
  } else { Skip "rtk binary not found - skipping official rtk uninstall (file fallback in section 3 still runs)" }
  # rtk binaries: ~/.knocode/bin prebuilt (current) + legacy ~/bin + legacy cargo install
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



  # 3c. Opencode plugin bundle -- copied into GLOBAL ~/.config/opencode/node_modules
  # by the release installer (self-contained dist, no npm). Legacy npm installs
  # may also have left package.json deps behind - those are the user's own file
  # now (knocode no longer creates it), so only the bundle dirs are removed.
  Info "Removing opencode plugin bundle (opencode-knocode)..."
  $ocGlobalDir = Join-Path $env:USERPROFILE ".config\opencode"
  $opencodeNodeModules = @(
    (Join-Path $ocGlobalDir "node_modules\opencode-knocode"),
    (Join-Path $Root ".opencode\node_modules\opencode-knocode"),
    (Join-Path $env:USERPROFILE ".cache\opencode\node_modules\opencode-knocode")
  )
  foreach ($p in $opencodeNodeModules) {
    $display = $p -replace [regex]::Escape($Root + "\"), "" -replace [regex]::Escape($Root + "/"), ""
    if ($display -eq $p) { $display = $p -replace [regex]::Escape($env:USERPROFILE + "\"), "~\" }
    if (Test-Path $p) {
      if ($PSCmdlet.ShouldProcess($display, "Remove-Item")) {
        try { Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop; Ok "removed $display (opencode plugin bundle)" } catch { Warn "failed to remove $display : $_" }
      } else { Skip "would remove $display" }
    } else { Skip "not found $display" }
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
if (Test-Path "$env:USERPROFILE\.config\opencode\plugins\rtk.ts") { Warn "global plugin still present at $env:USERPROFILE\.config\opencode\plugins\rtk.ts - may need manual removal or restart opencode" }
if ($doRemoveRepo -and (Test-Path "$Root\.opencode\plugins\knocode.ts")) { Warn "repository plugin still present at .opencode/plugins/knocode.ts even after --RemoveRepo" }
if ($doRemoveRepo -and (Test-Path "$Root\.opencode\plugins\rtk.ts")) { Warn "repository plugin still present at .opencode/plugins/rtk.ts even after --RemoveRepo" }
if ($doRemoveExternal) {
  # Legacy installs (pre-bundle) created this file - knocode no longer touches it.
  if ((Test-Path "$env:USERPROFILE\.config\opencode\package.json") -and (Select-String -LiteralPath "$env:USERPROFILE\.config\opencode\package.json" -Pattern "opencode-knocode" -Quiet -ErrorAction SilentlyContinue)) { Warn "global ~\.config\opencode\package.json still references opencode-knocode (legacy install) - remove the dep manually if desired" }
}
