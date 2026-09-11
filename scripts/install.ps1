#Requires -Version 5.1
<#
.SYNOPSIS
  Knocode installer minimal (Windows PowerShell 5.1)
  Installs minimal v1 stack + uses prebuilt knocode (no compile/test). Idempotent - re-run to update.

.DESCRIPTION
  Minimal v1: Node >=20, Python+pip, Git, SQLite(bundled), tree-sitter/ripgrep/tantivy/tiktoken embedded,
         RTK (optional) - no Rust needed (prebuilt binaries; compile via scripts/compile.*)
  Agent integrations: OpenCode, Copilot (VS Code), Claude Code, Cursor, Gemini CLI,
  Codex, Cline - select one or more at install (default: all). Prebuilt: target/release/knocode.exe + knocode-daemon.exe are used directly.

.PARAMETER SkipBuild
  Deprecated - build is always skipped (prebuilt binary at target/release/knocode.exe is used). Kept for compat.

.PARAMETER Agents
  Comma-separated agent list to wire, e.g. "-Agents opencode". Valid: opencode, copilot, claude, cursor, gemini, codex, cline.

.PARAMETER AllAgents
  Install agent integrations for all supported agents (default when interactive prompt is not possible).

.PARAMETER NoAgents
  Skip agent integrations entirely (binaries + config + doctor only).

.PARAMETER WithRtk
  Install and wire RTK without prompting.

.PARAMETER NoRtk
  Skip RTK entirely (binary download + per-agent wiring).

.PARAMETER SkipPrereqs
  Do not auto-install missing prerequisites (Node.js, Git) - only warn/fail.

.PARAMETER LogVerbosity
  Log verbosity: 0 quiet (errors only) / 1 normal (default) / 2 verbose (log every daemon call).
  Persisted as KNOCODE_LOG_LEVEL (user env: error/info/debug) and [logging] level in
  ~/.config/knocode/config.toml (the daemon config). Asked interactively when omitted.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -Agents opencode
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -AllAgents
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -NoAgents
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -LogVerbosity 2
#>
param([switch]$SkipBuild, [string]$Agents = "", [switch]$AllAgents, [switch]$NoAgents, [switch]$WithRtk, [switch]$NoRtk, [switch]$SkipPrereqs, [string]$LogVerbosity = "")

$ErrorActionPreference = "Stop"
# Always English in scripts (avoid localized ShouldProcess/WhatIf)
try { [System.Threading.Thread]::CurrentThread.CurrentUICulture = [System.Globalization.CultureInfo]::GetCultureInfo('en-US'); [System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::GetCultureInfo('en-US') } catch {}
# UTF-8 for native output: tools like rtk emit UTF-8; without this, PowerShell 5.1
# decodes their stdout with the console ANSI codepage and relayed lines show mojibake
# (em-dash renders as "A with circumflex" garbage). No-op on PS7 / already-UTF8
# consoles; never fatal when there is no console handle (output redirected).
try {
  if ([Console]::OutputEncoding.CodePage -ne 65001) { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 }
  $OutputEncoding = [System.Text.Encoding]::UTF8
} catch {}
$Root = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $Root

function Test-Cmd($cmd) { $null -ne (Get-Command $cmd -ErrorAction SilentlyContinue) }
function Info($m) { Write-Host "[knocode] $m" -ForegroundColor Cyan }
function Ok($m) { Write-Host "  [OK] $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkGray }
# UTF-8 WITHOUT BOM: PowerShell 5.1 `Set-Content -Encoding UTF8` writes a BOM,
# which breaks strict JSON/TOML parsers (seen: Cline rejecting MCP configs).
# All machine-written configs go through this helper. No -NoNewline needed:
# pass the exact final text.
function Set-Utf8NoBom($path, $content) { [IO.File]::WriteAllText($path, [string]$content, (New-Object System.Text.UTF8Encoding($false))) }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; throw $m }

# Prerequisite auto-install helpers - the installer installs what it needs,
# nothing depends on the user (use -SkipPrereqs to opt out).
function Add-ToUserPath($dir) {
  try {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $userPath) { $userPath = "" }
    $entries = $userPath -split ';' | Where-Object { $_ -ne '' }
    if ($entries -notcontains $dir) { [Environment]::SetEnvironmentVariable('Path', (($entries + $dir) -join ';'), 'User') }
  } catch { Warn "could not persist PATH for $dir : $_" }
  if (($env:Path -split ';') -notcontains $dir) { $env:Path = "$dir;$env:Path" }
}
function Install-NodeIfMissing {
  Info "Installing Node.js LTS (per-user, no admin)..."
  try {
    $idx = Invoke-RestMethod -Uri "https://nodejs.org/dist/index.json" -UseBasicParsing -TimeoutSec 30
    $lts = $idx | Where-Object { $_.lts } | Select-Object -First 1
    if (-not $lts) { throw "could not determine Node.js LTS version" }
    $ver = $lts.version
    $tmp = Join-Path $env:TEMP ("knocode_node_" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
      $zip = Join-Path $tmp "node.zip"
      Invoke-WebRequest -Uri "https://nodejs.org/dist/$ver/node-$ver-win-x64.zip" -OutFile $zip -UseBasicParsing
      $ex = Join-Path $tmp "x"
      Expand-Archive -LiteralPath $zip -DestinationPath $ex -Force
      $nodeRoot = Get-ChildItem -LiteralPath $ex -Directory | Select-Object -First 1
      if (-not $nodeRoot) { throw "Node.js archive is malformed" }
      $nodeDir = Join-Path $env:LOCALAPPDATA "Programs\nodejs"
      New-Item -ItemType Directory -Force -Path $nodeDir | Out-Null
      Copy-Item -Path (Join-Path $nodeRoot.FullName "*") -Destination $nodeDir -Recurse -Force
      Add-ToUserPath $nodeDir
      Ok "Node.js $ver installed to $nodeDir"
    } finally { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
  } catch { Warn "Node.js auto-install failed: $($_.Exception.Message)" }
}
function Install-GitIfMissing {
  Info "Installing Git for Windows (per-user, silent)..."
  try {
    $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/git-for-windows/git/releases/latest" -Headers @{ "User-Agent" = "knocode-installer" } -UseBasicParsing
    $asset = $rel.assets | Where-Object { $_.name -match "^\d+\.\d+\.\d+.*-64-bit\.exe$" } | Select-Object -First 1
    if (-not $asset) { throw "no 64-bit installer asset found in $($rel.tag_name)" }
    $exe = Join-Path $env:TEMP "git-setup.exe"
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $exe -UseBasicParsing
    $p = Start-Process -FilePath $exe -ArgumentList "/VERYSILENT", "/NORESTART", "/NOCANCEL", "/SP-", "/CURRENTUSER" -Wait -PassThru
    Remove-Item -LiteralPath $exe -Force -ErrorAction SilentlyContinue
    if ($p.ExitCode -ne 0) { throw "git installer exited with code $($p.ExitCode)" }
    $gitCmd = Join-Path $env:LOCALAPPDATA "Programs\Git\cmd"
    if (Test-Path (Join-Path $gitCmd "git.exe")) { Add-ToUserPath $gitCmd }
    Ok "Git installed (per-user, $($rel.tag_name))"
  } catch { Warn "Git auto-install failed: $($_.Exception.Message)" }
}

# --- Agent catalog & selection -------------------------------------------------
# opencode/copilot have bespoke integrations (plugin / hooks). claude, cursor,
# gemini, codex and cline share the universal MCP+skill wiring below.
$AgentCatalog = @("opencode", "copilot", "claude", "cursor", "gemini", "codex", "cline")
$UniversalAgents = @("claude", "cursor", "gemini", "codex", "cline")

function Select-Agents {
  if ($NoAgents) { return @() }
  if ($Agents -ne "") {
    $sel = @()
    foreach ($a in ($Agents -split ",")) {
      $a = $a.Trim().ToLower()
      if ($AgentCatalog -contains $a) { $sel += $a } else { Warn "unknown agent '$a' - valid: $($AgentCatalog -join ', ')" }
    }
    if ($sel.Count -eq 0) { Fail "no valid agents in -Agents ('$Agents')" }
    return ($sel | Select-Object -Unique)
  }
  if ($AllAgents) { return @($AgentCatalog) }

  # Interactive multi-select when stdin is a console; default to ALL otherwise
  $interactive = $true
  try { if ([Console]::IsInputRedirected) { $interactive = $false } } catch { $interactive = $false }
  if (-not $interactive) {
    Info "non-interactive run - installing agent integrations for ALL agents (use -Agents opencode or -NoAgents to change)"
    return @($AgentCatalog)
  }
  Info "Which agent integrations should be installed? (default Yes for each)"
  $sel = @()
  foreach ($a in $AgentCatalog) {
    $r = Read-Host "  Wire up $a ? [Y/n]"
    if ($r -eq "" -or $r -match "^(y|yes)$") { $sel += $a } else { Skip "$a skipped" }
  }
  if ($sel.Count -eq 0) { Info "no agent integrations selected" }
  return $sel
}

Info "Knocode installer"
$agentSel = @(Select-Agents)
if ($agentSel.Count -gt 0) { Info "Agent integrations: $($agentSel -join ', ')" } else { Info "Agent integrations: none" }

# --- Log verbosity selection (0 quiet / 1 normal / 2 verbose) --------------------
# Shared knob: KNOCODE_LOG_LEVEL feeds BOTH the daemon ([logging] level fallback /
# env override) and the agent plugins (0 = errors only, 1 = outcome lines, 2 = one
# line per daemon call).
function Select-LogVerbosity {
  if ($LogVerbosity -ne "") {
    if ("0", "1", "2" -notcontains $LogVerbosity) { Warn "invalid -LogVerbosity '$LogVerbosity' - valid: 0, 1, 2"; return "1" }
    return $LogVerbosity
  }
  $interactive = $true
  try { if ([Console]::IsInputRedirected) { $interactive = $false } } catch { $interactive = $false }
  if (-not $interactive) { return "1" }
  $r = Read-Host "  Log verbosity? [0] quiet (errors only) [1] normal [2] verbose (every daemon call) [1]"
  switch ($r) { "0" { return "0" } "2" { return "2" } default { return "1" } }
}
$verbosity = Select-LogVerbosity
switch ($verbosity) {
  "0" { $logLevelStr = "error"; $verbosityDesc = "quiet (errors only)" }
  "2" { $logLevelStr = "debug"; $verbosityDesc = "verbose (every daemon call logged)" }
  default { $logLevelStr = "info"; $verbosityDesc = "normal"; $verbosity = "1" }
}
Info "Log verbosity: $verbosity ($verbosityDesc)"

# 0a. Stop any running daemon/CLI up front - later steps REPLACE binaries (~\.knocode\bin)
# and a locked exe would fail the copy. The fresh daemon is restarted at the end (step 4).
foreach ($procName in @("knocode-daemon", "knocode")) {
  Get-Process -Name $procName -ErrorAction SilentlyContinue | ForEach-Object {
    try { Stop-Process -Id $_.Id -Force -ErrorAction Stop; Info "stopped $procName PID $($_.Id)" } catch {}
  }
}

# 0. Prereqs - no Rust needed: knocode ships prebuilt (target/release) and the installer does not
#    compile. Source builds use scripts/compile.* (or CI). Rust/clippy were removed from the
#    installer when it stopped compiling.

if (-not (Test-Cmd node)) {
  if ($SkipPrereqs) { Warn "node not found - install Node >=20 https://nodejs.org (or re-run without -SkipPrereqs)" } else { Install-NodeIfMissing }
} else { Ok "node $(node --version)" }
if (-not (Test-Cmd python) -and -not (Test-Cmd python3)) {
  Info "python not found - installing Python 3.13..."
  try {
    if (Get-Command winget -ErrorAction SilentlyContinue) {
      winget install --id Python.Python.3.13 -e --accept-package-agreements --accept-source-agreements --silent 2>&1 | Out-Null
      $pyPaths = @("$env:LOCALAPPDATA\Programs\Python\Python313\python.exe", "$env:LOCALAPPDATA\Programs\Python\Python313\Scripts\python.exe", "C:\Python313\python.exe")
      foreach ($p in $pyPaths) { if (Test-Path $p) { $env:Path = "$(Split-Path $p -Parent);$(Split-Path $p -Parent)\Scripts;$env:Path"; break } }
    } else {
      # Fallback: download official installer
      $pyUrl = "https://www.python.org/ftp/python/3.13.2/python-3.13.2-amd64.exe"
      $pyInst = "$env:TEMP\python-3.13.2-amd64.exe"
      Invoke-WebRequest -Uri $pyUrl -OutFile $pyInst -UseBasicParsing
      & $pyInst /quiet InstallAllUsers=0 PrependPath=1 Include_test=0 2>&1 | Out-Null
      Start-Sleep -Seconds 5
      $env:Path = "$env:LOCALAPPDATA\Programs\Python\Python313\Scripts;$env:LOCALAPPDATA\Programs\Python\Python313;$env:Path"
    }
    # Refresh command cache
    if (Test-Cmd python -or Test-Cmd python3) { Ok "python $((python --version 2>&1) -join ' ')" } else { Warn "python install attempted but python not on PATH - install manually: https://www.python.org/downloads/ (check 'Add to PATH')" }
  } catch { Warn "python auto-install failed - $_ (install manually: https://www.python.org/downloads/)" }
} else { Ok "python $((python --version 2>&1) -join ' ')" }
if (-not (Test-Cmd git)) {
  if ($SkipPrereqs) { Fail "git not found" } else { Install-GitIfMissing; if (-not (Test-Cmd git)) { Fail "git not found after auto-install" } }
} else { Ok "git $(git --version)" }


# 1. Use prebuilt knocode (no compile/test - use repository binary)
if ($SkipBuild) { Info "Skipping build check (--SkipBuild)" }
Info "Checking prebuilt knocode..."
$prebuilt = Join-Path $Root "target\release\knocode.exe"
$prebuiltDaemon = Join-Path $Root "target\release\knocode-daemon.exe"
# Fallback: cargo may use a global target dir (e.g. ~/.cargo/target) when
# CARGO_TARGET_DIR or [build] target-dir is set in .cargo/config.toml.
# Detect via cargo metadata and sync binaries to repo-local target/release/:
# copy when missing, refresh when the cargo-built binary is newer.
$cargoReleaseDir = $null
try {
  $metaJson = & cargo metadata --no-deps --format-version 1 2>$null | Out-String
  if ($LASTEXITCODE -eq 0 -and $metaJson) {
    $cargoTargetDir = ($metaJson | ConvertFrom-Json).target_directory
    if ($cargoTargetDir -and (Test-Path $cargoTargetDir)) { $cargoReleaseDir = Join-Path $cargoTargetDir "release" }
  }
} catch {}
function Sync-Prebuilt([string]$Name) {
  $dest = Join-Path $Root "target\release\$Name.exe"
  $src = if ($cargoReleaseDir) { Join-Path $cargoReleaseDir "$Name.exe" } else { $null }
  if (-not (Test-Path $dest)) {
    if ($src -and (Test-Path $src)) {
      New-Item -ItemType Directory -Force -Path (Split-Path $dest) | Out-Null
      Copy-Item -LiteralPath $src -Destination $dest -Force
      Info "Copied $Name from cargo target dir ($cargoReleaseDir) -> target/release/"
    }
    return
  }
  if ($src -and (Test-Path $src) -and ((Get-Item $src).LastWriteTime -gt (Get-Item $dest).LastWriteTime)) {
    Copy-Item -LiteralPath $src -Destination $dest -Force
    Info "Refreshed stale $Name from cargo target dir ($cargoReleaseDir) -> target/release/"
  }
}
Sync-Prebuilt "knocode"
Sync-Prebuilt "knocode-daemon"
if (Test-Path $prebuilt) { Ok "knocode at target/release/knocode.exe" } else { Warn "knocode binary not found at target/release/knocode.exe - build manually: cargo build --release"; Fail "prebuilt knocode.exe missing - expected at target/release/knocode.exe" }
if (Test-Path $prebuiltDaemon) { Ok "knocode-daemon at target/release/knocode-daemon.exe" } else { Warn "knocode-daemon not found at target/release/knocode-daemon.exe" }

# 1b. TASK-037: ship binaries to %USERPROFILE%\.knocode\bin + persist on the USER PATH,
# so `knocode --version` and the daemon keep working from ANY directory/shell even if this
# repo checkout is moved or cleaned (cargo clean / -RemoveRepo). Idempotent re-run.
$binDir = Join-Path $env:USERPROFILE ".knocode\bin"
$installedCli = Join-Path $binDir "knocode.exe"
$installedDaemon = Join-Path $binDir "knocode-daemon.exe"
try {
  New-Item -ItemType Directory -Force -Path $binDir | Out-Null
  Copy-Item -LiteralPath $prebuilt -Destination $installedCli -Force -ErrorAction Stop
  Ok "knocode.exe installed to $installedCli"
} catch { Warn "failed to copy knocode.exe to ${binDir}: $_"; $installedCli = $prebuilt }
if (Test-Path $prebuiltDaemon) {
  try {
    Copy-Item -LiteralPath $prebuiltDaemon -Destination $installedDaemon -Force -ErrorAction Stop
    Ok "knocode-daemon.exe installed to $installedDaemon"
  } catch {
    Warn "failed to copy knocode-daemon.exe to ${binDir}: $_"
    if (-not (Test-Path $installedDaemon)) { $installedDaemon = $prebuiltDaemon }
  }
}
# Persist on the user PATH (HKCU Environment) — append only when missing (idempotent)
try {
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if ($null -eq $userPath) { $userPath = "" }
  $entries = $userPath -split ';' | Where-Object { $_ -ne '' }
  if ($entries -notcontains $binDir) {
    $newUserPath = ($entries + $binDir) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
    Info "Added $binDir to USER PATH (persisted in HKCU Environment)"
  } else { Ok "$binDir already on USER PATH" }
} catch { Warn "could not persist USER PATH: $_" }
# Current session PATH so subsequent steps resolve knocode without the repo checkout
if (($env:Path -split ';') -notcontains $binDir) { $env:Path = "$binDir;$env:Path" }

# 2. Verify installation (doctor)
# NOTE: `knocode init` / `knocode index` are NOT run here on purpose - they bootstrap the
# repository they run IN (per-repo .knocode/ + index), which is meaningless for the knocode
# source checkout itself. Run them inside each repo you want analyzed.
Info "Verifying installation (doctor)..."
$prevEA2 = $ErrorActionPreference; $ErrorActionPreference = "Continue"
try { & $installedCli doctor } catch {}
$ErrorActionPreference = $prevEA2

# 1c. Persist log verbosity (shared KNOCODE_LOG_LEVEL knob)
#     a) user env var (agents/plugins read it regardless of shell),
#     b) [logging] level in the USER config (~/.config/knocode/config.toml) so the
#        daemon honors it even when started outside a shell that has the var.
#     Env wins over config for the daemon; config keeps it discoverable via `knocode doctor`.
$userCfgDir = Join-Path $env:USERPROFILE ".config\knocode"
$userCfg = Join-Path $userCfgDir "config.toml"
try {
  New-Item -ItemType Directory -Force -Path $userCfgDir | Out-Null
  if (Test-Path $userCfg) {
    $cfgText = Get-Content -LiteralPath $userCfg -Raw
    $line = 'level = "' + $logLevelStr + '"'
    if ($cfgText -match '(?m)^\s*level\s*=') {
      # Replace the WHOLE line — '$1' alone (group 1 = leading indent) + full line would
      # duplicate the key ('level = level = "x"') and produce TOML the daemon can't parse.
      $cfgText = [regex]::Replace($cfgText, '(?m)^\s*level\s*=.*$', $line)
    } elseif ($cfgText -match '(?m)^\[logging\]') {
      $cfgText = [regex]::Replace($cfgText, '(?m)^(\[logging\]\r?\n)', ('${1}' + $line + "`n"))
    } else {
      if (-not $cfgText.EndsWith("`n")) { $cfgText += "`n" }
      $cfgText += "`n[logging]`n$line`n"
    }
    Set-Utf8NoBom $userCfg $cfgText
    Ok "user config [logging] level = $logLevelStr ($userCfg)"
  } else {
    $newCfg = @"
[logging]
level = "$logLevelStr"
file_path = "~/.knocode/logs/knocode.log"
max_size_mb = 100
retention_days = 7
"@
    Set-Utf8NoBom $userCfg $newCfg
    Ok "user config written ($userCfg, logging.level = $logLevelStr)"
  }
} catch { Warn "could not update $userCfg : $_" }
# User env var: plugins/agents read KNOCODE_LOG_LEVEL in any shell; also set it for
# this session so the daemon started below (step 4) inherits the chosen level.
$envLogLevel = switch ($verbosity) { "0" { "error" } "2" { "debug" } default { "info" } }
try {
  [Environment]::SetEnvironmentVariable('KNOCODE_LOG_LEVEL', $envLogLevel, 'User')
  $env:KNOCODE_LOG_LEVEL = $envLogLevel
  if ($verbosity -ne "1") {
    Ok "KNOCODE_LOG_LEVEL=$envLogLevel persisted in user environment (HKCU)"
  } else {
    Ok "KNOCODE_LOG_LEVEL=info set (default)"
  }
} catch { Warn "could not persist KNOCODE_LOG_LEVEL: $_" }

# =====================================================================================
# 3. Agent integrations (OpenCode / Copilot) - selected above
# =====================================================================================
if ($agentSel.Count -gt 0) {
  $ocGlobalDir = Join-Path $env:USERPROFILE ".config\opencode"

  # --- OpenCode: global plugin + skill (~/.config/opencode) ---
  if ($agentSel -contains "opencode") {
    Info "Configuring opencode plugin (GLOBAL: ~/.config/opencode)..."
    New-Item -ItemType Directory -Force -Path $ocGlobalDir | Out-Null
    $ocGlobalCfg = Join-Path $ocGlobalDir "opencode.jsonc"
    # NOTE: file:// spec (not the bare npm name) — opencode-knocode is not
    # published to the npm registry, and a bare spec makes the opencode
    # loader fail at the install stage so the plugin never loads. The
    # file:// URL loads the local build directly (self-contained esbuild
    # bundle at packages/opencode-knocode/dist/index.js - no npm step needed).
    $pluginFileUrl = "file://" + ((Join-Path $Root "packages\opencode-knocode") -replace '\\','/')
    $opencodeJsonc = @"
{
    "`$schema": "https://opencode.ai/config.json",
    "plugin": ["$pluginFileUrl"]
}
"@
    try { Set-Utf8NoBom $ocGlobalCfg $opencodeJsonc; Ok "opencode plugin at $ocGlobalCfg" } catch { Warn "failed to write $ocGlobalCfg : $_" }
    # Remove legacy paths
    $globalPlugin = "$env:USERPROFILE\.config\opencode\plugins\knocode.ts"
    if (Test-Path $globalPlugin) { try { Remove-Item -LiteralPath $globalPlugin -Force } catch {} }
    $localPlugin = Join-Path $Root ".opencode\plugins\knocode.ts"
    if (Test-Path $localPlugin) { try { Remove-Item -LiteralPath $localPlugin -Force } catch {} }
    foreach ($f in @((Join-Path $Root ".opencode\opencode.jsonc"), (Join-Path $Root ".opencode\opencode.json"), (Join-Path $Root ".opencode\package.json"), (Join-Path $Root ".opencode\package-lock.json"))) {
      if (Test-Path $f) { try { Remove-Item -LiteralPath $f -Force } catch {} }
    }
    # dist/index.js is a self-contained esbuild bundle (see packages/opencode-knocode) -
    # no npm step at install time. The file:// URL below loads the local build directly.
    $pluginDir = Join-Path $Root "packages\opencode-knocode"
    $pluginDist = Join-Path $pluginDir "dist\index.js"
    if (Test-Path $pluginDist) {
      Ok "opencode-knocode dist at packages/opencode-knocode/dist/index.js (self-contained bundle)"
    } elseif (Test-Path $pluginDir) {
      Warn "opencode-knocode dist not built - run: cd packages/opencode-knocode; npm install; npm run build"
    } else { Warn "packages/opencode-knocode not found - skipping plugin install" }
    # NOTE: no skill install for opencode — the plugin injects context
    # transparently, so a skill is unnecessary (legacy
    # ~/.config/opencode/skills/knocode is removed by the uninstaller).
    Info "Restart opencode to load the plugin (daemon http://127.0.0.1:9527)"
  }

  # --- Copilot (VS Code): NO user-level MCP registration ---
  # The knocode MCP is internal to the Copilot Agent Plugin (plugin mcp.json ->
  # ${PLUGIN_ROOT}/servers/knocode-mcp.mjs) and is never exposed globally.
  # Clean up any knocode entry left in VS Code's user mcp.json by previous installs.
  if ($agentSel -contains "copilot") {
    try {
      $vscodeMcp = Join-Path $env:APPDATA "Code\User\mcp.json"
      if (Test-Path $vscodeMcp) {
        try {
          $existing = Get-Content -LiteralPath $vscodeMcp -Raw | ConvertFrom-Json
          if ($existing.servers -and $existing.servers.knocode) {
            $existing.servers.PSObject.Properties.Remove('knocode')
            Set-Utf8NoBom $vscodeMcp ($existing | ConvertTo-Json -Depth 10)
            Ok "removed legacy knocode MCP entry from $vscodeMcp (MCP is plugin-internal only)"
          }
        } catch { Skip "could not clean knocode entry from $vscodeMcp" }
      }
    } catch { Warn "failed to clean VS Code Copilot MCP config: $_" }

    # --- Copilot Agent Plugin (hooks: UserPromptSubmit/PreToolUse) ---
    # Deploy to %USERPROFILE%\.knocode\copilot-plugin (repo-independent, survives repo moves).
    $pluginSrc = Join-Path $Root "packages\knocode-copilot-plugin"
    $pluginDst = Join-Path $env:USERPROFILE ".knocode\copilot-plugin"
    if (Test-Path (Join-Path $pluginSrc "plugin.json")) {
      try {
        New-Item -ItemType Directory -Force -Path $pluginDst | Out-Null
        # Fresh copy (idempotent update): clear destination first
        if (Test-Path $pluginDst) { Remove-Item -LiteralPath $pluginDst -Recurse -Force -ErrorAction SilentlyContinue }
        New-Item -ItemType Directory -Force -Path $pluginDst | Out-Null
        Copy-Item -Path (Join-Path $pluginSrc "*") -Destination $pluginDst -Recurse -Force -Exclude "node_modules"
        Ok "Copilot Agent Plugin deployed to $pluginDst (hooks + MCP)"
      } catch { Warn "failed to deploy Copilot Agent Plugin: $_" }
    } else { Warn "packages/knocode-copilot-plugin not found - skipping Agent Plugin deploy" }

    # --- Copilot hooks (user-level ~/.copilot/hooks) ---
    # VS Code/Copilot does NOT discover agent plugins from ~/.knocode — the bundle
    # below is only the hook-script home. Registration happens by writing a hooks
    # file into ~/.copilot/hooks/ (same mechanism RTK uses for rtk-rewrite.json),
    # with an absolute script path (forward slashes: JSON-safe, Windows-fine).
    # UserPromptSubmit injects the context (fires every turn, incl. tool-less
    # answers); PreToolUse is the consume-once retry when submit failed.
    if (Test-Path (Join-Path $pluginDst "scripts\knocode-hook.mjs")) {
      try {
        $hookScript = (Join-Path $pluginDst "scripts\knocode-hook.mjs") -replace '\\', '/'
        $knocodeHooksJson = @"
{
  "version": 1,
  "hooks": {
    "UserPromptSubmit": [
      {
        "type": "command",
        "command": "node \"$hookScript\" user-prompt-submit",
        "timeout": 5
      }
    ],
    "PreToolUse": [
      {
        "type": "command",
        "command": "node \"$hookScript\" pre-tool-use",
        "timeout": 15
      }
    ]
  }
}
"@
        $copilotHooksDir = Join-Path $env:USERPROFILE ".copilot\hooks"
        New-Item -ItemType Directory -Force -Path $copilotHooksDir | Out-Null
        $knocodeHooksFile = Join-Path $copilotHooksDir "knocode-context.json"
        [IO.File]::WriteAllText($knocodeHooksFile, $knocodeHooksJson, (New-Object System.Text.UTF8Encoding($false)))
        Ok "Copilot hooks registered at $knocodeHooksFile (UserPromptSubmit + PreToolUse)"
      } catch { Warn "failed to write Copilot hooks file: $_" }
    } else { Warn "knocode-hook.mjs not deployed - skipping Copilot hooks registration" }

    # NOTE: no skill install for Copilot — context flows through the hooks,
    # so a skill is unnecessary (legacy ~/.copilot/skills/knocode is removed
    # by the uninstaller). Universal agents below cover MCP+skill consumers.

    # --- @knocode chat participant extension (VSIX via `code` CLI) ---
    $extDir = Join-Path $Root "packages\vscode-copilot-knocode"
    if (Test-Path (Join-Path $extDir "package.json")) {
      $vsix = $null
      if ((Test-Cmd npm) -and -not (Test-Path (Join-Path $extDir "dist\extension.js"))) {
        Info "Building vscode-copilot-knocode extension..."
        Push-Location $extDir
        try {
          & npm install --silent 2>&1 | Out-Null
          & npm run build --silent 2>&1 | Out-Null
        } catch {}
        Pop-Location
      }
      # Package a VSIX if none exists yet (requires vsce / npx)
      if (-not (Get-ChildItem -Path $extDir -Filter "*.vsix" -ErrorAction SilentlyContinue)) {
        if (Test-Cmd npm) {
          Push-Location $extDir
          try {
            & npx --yes @vscode/vsce package --no-dependencies 2>&1 | Out-Null
          } catch {}
          Pop-Location
        }
      }
      $vsix = Get-ChildItem -Path $extDir -Filter "*.vsix" -ErrorAction SilentlyContinue | Select-Object -First 1
      if ($vsix) {
        if (Test-Cmd code) {
          Info "Installing @knocode VS Code extension (code --install-extension)..."
          $prevEA4 = $ErrorActionPreference; $ErrorActionPreference = "Continue"
          try { & code --install-extension $vsix.FullName --force 2>&1 | Out-Null; Ok "@knocode extension installed from $($vsix.Name) - reload VS Code to activate" }
          catch { Warn "code CLI install failed - install manually: code --install-extension $($vsix.FullName)" }
          $ErrorActionPreference = $prevEA4
        } else { Warn "VS Code 'code' CLI not on PATH - install manually: code --install-extension $($vsix.FullName)" }
      } else { Warn "vscode-copilot-knocode VSIX not built - open packages/vscode-copilot-knocode in VS Code and press F5, or run: npx @vscode/vsce package" }
    } else { Warn "packages/vscode-copilot-knocode not found - skipping @knocode extension install" }
  }

  # --- Universal agents (MCP + skill): claude, cursor, gemini, codex, cline ---
  # Skill copy per agent global folder + `knocode` MCP server entry in the agent's
  # global config. Fail-open per agent: one agent's failure never blocks others.
  $uniSel = @($agentSel | Where-Object { $UniversalAgents -contains $_ })
  if ($uniSel.Count -gt 0) {
    Info "Configuring universal agents (MCP + skill: $($uniSel -join ', '))..."
    # Shared stdio bridge (packages/knocode-mcp: zero-dep single file). Deployed
    # once to a repo-independent home; every agent MCP entry points at it.
    $mcpSrc = Join-Path $Root "packages\knocode-mcp\dist\index.js"
    if ((-not (Test-Path $mcpSrc)) -and (Test-Cmd npm)) {
      try { Push-Location (Join-Path $Root "packages\knocode-mcp"); & npm install --silent 2>&1 | Out-Null; & npm run build --silent 2>&1 | Out-Null; Pop-Location } catch { try { Pop-Location } catch {} }
    }
    $mcpDstDir = Join-Path $env:USERPROFILE ".knocode\mcp-server"
    $mcpDst = Join-Path $mcpDstDir "knocode-mcp.mjs"
    $haveBridge = $false
    if (Test-Path $mcpSrc) {
      try { New-Item -ItemType Directory -Force -Path $mcpDstDir | Out-Null; Copy-Item -LiteralPath $mcpSrc -Destination $mcpDst -Force; $haveBridge = $true; Ok "shared MCP bridge at $mcpDst" }
      catch { Warn "MCP bridge deploy failed: $_" }
    } else { Warn "packages/knocode-mcp dist not built - run: cd packages/knocode-mcp; npm install; npm run build (MCP entries skipped, skills still install)" }
    $mcpScript = ($mcpDst -replace '\\','/')

    function Install-UniversalSkill($agent, $destDir) {
      $src = Join-Path $Root ".knocode\skills-universal\knocode"
      if (-not (Test-Path (Join-Path $src "SKILL.md"))) { Warn ".knocode\skills-universal\knocode not found - skipping $agent skill"; return }
      try { New-Item -ItemType Directory -Force -Path (Split-Path $destDir) | Out-Null; if (Test-Path $destDir) { Remove-Item -LiteralPath $destDir -Recurse -Force }; Copy-Item -LiteralPath $src -Destination $destDir -Recurse -Force; Ok "$agent skill at $destDir" }
      catch { Warn "$agent skill copy failed: $_" }
    }
    function Merge-KnocodeMcpJson($agent, $configPath, $displayPath) {
      if (-not $script:haveBridge) { Skip "$agent MCP skipped (no bridge)"; return }
      try {
        $obj = $null
        if (Test-Path $configPath) {
          $raw = Get-Content -LiteralPath $configPath -Raw
          if ($raw -and $raw.Trim()) {
            try { $obj = $raw | ConvertFrom-Json -ErrorAction Stop }
            catch {
              $clean = ($raw -replace '(?m)^\s*//.*$','' -replace '/\*.*?\*/','') -replace ',\s*([\}\]])', '$1'
              $obj = $clean | ConvertFrom-Json -ErrorAction Stop
            }
          }
        }
        if (-not $obj) { $obj = [PSCustomObject]@{} }
        if (-not $obj.PSObject.Properties['mcpServers']) { $obj | Add-Member -NotePropertyName 'mcpServers' -NotePropertyValue ([PSCustomObject]@{}) }
        elseif ($obj.mcpServers -isnot [PSCustomObject]) { Warn "$agent MCP: existing mcpServers is not an object - skipping $displayPath"; return }
        $entry = [PSCustomObject]@{ command = "node"; args = @($script:mcpScript) }
        $obj.mcpServers | Add-Member -NotePropertyName 'knocode' -NotePropertyValue $entry -Force
        New-Item -ItemType Directory -Force -Path (Split-Path $configPath) | Out-Null
        Set-Utf8NoBom $configPath ($obj | ConvertTo-Json -Depth 10)
        Ok "$agent MCP registered in $displayPath"
      } catch { Warn "$agent MCP config failed ($displayPath): $_" }
    }

    if ($uniSel -contains "claude") {
      Install-UniversalSkill "claude" (Join-Path $env:USERPROFILE ".claude\skills\knocode")
      Merge-KnocodeMcpJson "claude" (Join-Path $env:USERPROFILE ".claude.json") "~\.claude.json (user scope)"
    }
    if ($uniSel -contains "cursor") {
      Install-UniversalSkill "cursor" (Join-Path $env:USERPROFILE ".cursor\skills\knocode")
      Merge-KnocodeMcpJson "cursor" (Join-Path $env:USERPROFILE ".cursor\mcp.json") "~\.cursor\mcp.json"
    }
    if ($uniSel -contains "gemini") {
      Install-UniversalSkill "gemini" (Join-Path $env:USERPROFILE ".gemini\skills\knocode")
      Merge-KnocodeMcpJson "gemini" (Join-Path $env:USERPROFILE ".gemini\settings.json") "~\.gemini\settings.json"
    }
    if ($uniSel -contains "codex") {
      $codexSkill = Join-Path $env:USERPROFILE ".knocode\skills\knocode"
      Install-UniversalSkill "codex" $codexSkill
      if ($haveBridge) {
        try {
          $codexCfg = Join-Path $env:USERPROFILE ".codex\config.toml"
          New-Item -ItemType Directory -Force -Path (Split-Path $codexCfg) | Out-Null
          $text = ""; if (Test-Path $codexCfg) { $text = Get-Content -LiteralPath $codexCfg -Raw }
          $skillPath = ($codexSkill -replace '\\','/')
          if ($text -notmatch '\[mcp_servers\.knocode\]') {
            $text += "`n[mcp_servers.knocode]`ncommand = `"node`"`nargs = [`"$mcpScript`"]`n"
          }
          if ($text -notmatch [regex]::Escape($skillPath)) {
            $text += "`n[[skills.config]]`npath = `"$skillPath`"`nenabled = true`n"
          }
          Set-Utf8NoBom $codexCfg $text
          Ok "codex MCP + skill registered in ~\.codex\config.toml"
        } catch { Warn "codex config failed: $_" }
      } else { Skip "codex MCP skipped (no bridge)" }
    }
    if ($uniSel -contains "cline") {
      Install-UniversalSkill "cline" (Join-Path $env:USERPROFILE ".cline\skills\knocode")
      Merge-KnocodeMcpJson "cline" (Join-Path $env:USERPROFILE ".cline\data\settings\cline_mcp_settings.json") "~\.cline\data\settings\cline_mcp_settings.json"
    }
  }
}

# 3a. RTK (optional external tool) - DEPENDS ON AGENT SELECTION
#     Offered AFTER agent wiring and ONLY when agent integrations were selected
#     (RTK without a wired agent has nothing to integrate with). Opt-in: -WithRtk
#     forces, -NoRtk skips, otherwise asked interactively (default No). RTK ships
#     its own OpenCode/Copilot integrations - knocode only installs the binary and
#     wires them via `rtk init -g` in section 3b (no reimplementation).
$rtkBinPath = Join-Path $env:USERPROFILE ".knocode\bin\rtk.exe"
$rtkCmd = $null
$rtkStatus = ""
if ($NoRtk) {
  $rtkStatus = "skipped (-NoRtk)"
}
elseif ($agentSel.Count -eq 0) {
  $rtkStatus = "skipped (no agent integrations selected)"
  if ($WithRtk) { Warn "-WithRtk was set but no agent integrations were selected - RTK not installed (re-run with -Agents opencode,copilot)" }
}
else {
  $wantRtk = [bool]$WithRtk
  if (-not $wantRtk) {
    $interactive = $true
    try { if ([Console]::IsInputRedirected) { $interactive = $false } } catch { $interactive = $false }
    if ($interactive) {
      $r = Read-Host "  Also install RTK for the selected agents ($($agentSel -join ', '))? [y/N]"
      $wantRtk = ($r -match "^(y|yes)$")
    }
    else { $rtkStatus = "skipped (non-interactive, use -WithRtk)" }
  }
  if ($wantRtk) {
    # Identity probe: the REAL rtk-ai/rtk has an `init` subcommand; name-collision
    # binaries on crates.io (e.g. "Rust Type Kit") exit 2 on it. Never trust a bare
    # `rtk` on PATH without this check.
    function Test-RealRtk([string]$cmd) {
      try { & $cmd init --help 2>&1 | Out-Null; return ($LASTEXITCODE -eq 0) } catch { return $false }
    }
    if ((Test-Cmd rtk) -and (Test-RealRtk "rtk")) { $rtkCmd = "rtk"; Ok "rtk $(rtk --version 2>&1 | Select-Object -First 1)" }
    elseif ((Test-Path $rtkBinPath) -and (Test-RealRtk $rtkBinPath)) { $env:Path = "$(Split-Path $rtkBinPath -Parent);$env:Path"; $rtkCmd = $rtkBinPath; Ok "rtk binary at $rtkBinPath" }
    else {
      $cmdInfo = Get-Command rtk -ErrorAction SilentlyContinue
      if ($cmdInfo) {
        $badRtk = $cmdInfo.Source
        Warn "'rtk' found on PATH but it is NOT rtk-ai/rtk (name collision, e.g. Rust Type Kit) - removing it so it cannot shadow the real RTK"
        if ($badRtk -like "*\.cargo\*") { cargo uninstall rtk 2>&1 | Out-Null }
        try { Remove-Item -LiteralPath $badRtk -Force -ErrorAction Stop } catch {}
        if (Test-Path $badRtk) { Warn "could not remove $badRtk - delete it manually or 'rtk' will still resolve to the wrong binary" }
      }
      $legacyRtk = "$env:USERPROFILE\bin\rtk.exe"
      if ((Test-Path $legacyRtk) -and -not (Test-Path $rtkBinPath)) {
        try { Copy-Item -LiteralPath $legacyRtk -Destination $rtkBinPath -Force; $env:Path = "$(Split-Path $rtkBinPath -Parent);$env:Path"; $rtkCmd = $rtkBinPath; Ok "migrated legacy $legacyRtk -> $rtkBinPath" } catch {}
      }
      else {
        $rtkAsset = "rtk-x86_64-pc-windows-msvc.zip"
        $rtkUrl = "https://github.com/rtk-ai/rtk/releases/latest/download/$rtkAsset"
        $rtkTmp = Join-Path $env:TEMP "rtk_dl"
        try {
          New-Item -ItemType Directory -Force -Path (Split-Path $rtkBinPath -Parent) | Out-Null
          if (Test-Path $rtkTmp) { Remove-Item -LiteralPath $rtkTmp -Recurse -Force -ErrorAction SilentlyContinue }
          New-Item -ItemType Directory -Force -Path $rtkTmp | Out-Null
          $rtkZip = Join-Path $rtkTmp $rtkAsset
          Info "  downloading rtk release ($rtkAsset)..."
          Invoke-WebRequest -Uri $rtkUrl -OutFile $rtkZip -UseBasicParsing
          $rtkExtract = Join-Path $rtkTmp "x"
          Expand-Archive -LiteralPath $rtkZip -DestinationPath $rtkExtract -Force
          $srcExe = Get-ChildItem -LiteralPath $rtkExtract -Recurse -Filter "rtk.exe" | Select-Object -First 1
          if ($srcExe) {
            Copy-Item -LiteralPath $srcExe.FullName -Destination $rtkBinPath -Force
            $env:Path = "$(Split-Path $rtkBinPath -Parent);$env:Path"
            $rtkCmd = $rtkBinPath
            Ok "rtk installed to $rtkBinPath (from GitHub release)"
          } else { Warn "rtk release archive did not contain rtk.exe" }
        } catch { Warn "rtk download failed: $_ - install manually from https://github.com/rtk-ai/rtk/releases" }
        finally { if (Test-Path $rtkTmp) { Remove-Item -LiteralPath $rtkTmp -Recurse -Force -ErrorAction SilentlyContinue } }
      }
    }
    if ($rtkCmd) { $rtkStatus = "installed" } elseif ($rtkStatus -eq "") { $rtkStatus = "failed" }
  }
  elseif ($rtkStatus -eq "") { $rtkStatus = "declined" }
}

# 3b. RTK agent wiring - RTK ships its own per-agent integrations (global hooks
#     for claude/cursor/gemini/codex/copilot, plugin for opencode; cline is
#     project-scoped .clinerules only). Hand off to RTK's own `rtk init` with the
#     documented per-agent flags. Fail-open: never blocks the knocode install.
if ($agentSel.Count -gt 0 -and $rtkCmd) {
  Info "Wiring RTK integrations for selected agents (external tool)..."
  if (-not (Test-Cmd rg)) {
    Warn "ripgrep (rg) not on PATH - some rtk filters need it (winget install BurntSushi.ripgrep.MSVC)"
  }
  # Per-agent RTK flags (rtk-ai/rtk): claude is the default global hook,
  # cursor/gemini/codex/copilot/opencode take their own global flags. Cline has
  # NO global integration (prompt-level `.clinerules`, project-scoped) — skipped
  # with guidance below. --auto-patch keeps every variant non-interactive.
  $rtkAgentArgs = @{
    "opencode" = @("init", "-g", "--opencode", "--auto-patch")
    "copilot"  = @("init", "-g", "--copilot", "--auto-patch")
    "claude"   = @("init", "-g", "--auto-patch")
    "cursor"   = @("init", "-g", "--agent", "cursor", "--auto-patch")
    "gemini"   = @("init", "-g", "--gemini", "--auto-patch")
    "codex"    = @("init", "-g", "--codex", "--auto-patch")
  }
  if ($agentSel -contains "cline") {
    Skip "cline has no global RTK integration - run 'rtk init --agent cline' inside each project you open with Cline (writes .clinerules)"
  }
  $rtkAgents = @($agentSel | Where-Object { $rtkAgentArgs.ContainsKey($_) })
  $n = 0
  foreach ($a in $rtkAgents) {
    $n++
    $rtkArgs = $rtkAgentArgs[$a]
    Info "  [$n/$($rtkAgents.Count)] wiring rtk for $a (runs: rtk $($rtkArgs -join ' ') - usually takes a few seconds)..."
    $prevEA = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try {
      # stdin closed + output shown: rtk never waits silently on the installer's stdin,
      # and the user sees progress instead of a frozen prompt if it needs time.
      $out = & $rtkCmd @rtkArgs 2>&1
      if ($LASTEXITCODE -eq 0) {
        Ok "rtk integration wired for $a (rtk $($rtkArgs -join ' '))"
        # Relay rtk output minus its "/!\ No hook installed" upsell: the global hook
        # is installed right after this loop; the filter stays in case rtk still
        # prints the warning (e.g. the hook install failed).
        $out | Where-Object { $_ -and $_.ToString().Trim() -and $_.ToString() -notmatch 'No hook installed' } | Select-Object -First 3 | ForEach-Object { Info "    $_" }
      }
      else { Warn "rtk init failed for $a (exit $LASTEXITCODE) - run manually: rtk $($rtkArgs -join ' ')"; $out | Select-Object -First 5 | ForEach-Object { Info "    $_" } }
    } catch { Warn "rtk init failed for $a : $_" }
    $ErrorActionPreference = $prevEA
  }

  # ── Global hook - `rtk init -g --auto-patch` registers RTK's compression hook ──
  # (Claude-style hook + RTK.md) so token savings also apply outside the wired
  # agents. Fail-open: never blocks the install. `$null |` closes stdin so rtk
  # never waits on the installer's stdin. MUST run before the rtk.ts PATCH below:
  # init regenerates the plugin file (and resurrects the `which rtk` probe).
  Info "Installing RTK global hook (rtk init -g --auto-patch)..."
  $prevEA = $ErrorActionPreference; $ErrorActionPreference = "Continue"
  try {
    $out = $null | & $rtkCmd init -g --auto-patch 2>&1
    if ($LASTEXITCODE -eq 0) {
      Ok "rtk global hook installed (rtk init -g --auto-patch)"
      $out | Where-Object { $_ -and $_.ToString().Trim() } | Select-Object -First 3 | ForEach-Object { Info "    $_" }
    }
    else { Warn "rtk global hook install failed (exit $LASTEXITCODE) - run manually: rtk init -g --auto-patch"; $out | Select-Object -First 5 | ForEach-Object { Info "    $_" } }
  } catch { Warn "rtk global hook install failed : $_" }
  $ErrorActionPreference = $prevEA

  # ── PATCH: rtk.ts binary probe - `which rtk` is Unix-only ───────────────
  # RTK's generated OpenCode plugin (rtk init --opencode) probes with `which`,
  # which does not exist on Windows - the plugin would disable itself even
  # though rtk is installed. Replace the probe with a portable `rtk --version`
  # call (idempotent: only rewrites when the old probe is still present).
  $rtkPlugin = Join-Path $env:USERPROFILE ".config\opencode\plugins\rtk.ts"
  if (Test-Path $rtkPlugin) {
    try {
      $content = Get-Content -LiteralPath $rtkPlugin -Raw
      if ($content -match 'which rtk') {
        $content = $content -replace '`which rtk`', '`rtk --version`'
        Set-Utf8NoBom $rtkPlugin $content
        Ok "PATCH: rtk.ts probe now uses 'rtk --version' (Windows-safe)"
      }
    } catch { Warn "PATCH of rtk.ts failed: $($_.Exception.Message)" }
  }

  Info "RTK wiring done."
}

# 4. Start daemon - knocode must be in RUNNING state after installation
# TASK-037: launch the daemon from ~\.knocode\bin (installed copy) so the runtime keeps
# working if the repo is moved/cleaned — NOT from target\release.
# Multi-repo daemon: launched WITHOUT --repo, it starts repo-neutral (no CWD binding,
# no startup index). Repositories are indexed lazily on their first request (the agent
# plugin sends repository_path) and watched from then on, so working from ~\.knocode
# is correct here — it no longer causes the daemon to index its own home directory.
function Test-DaemonHealth {
  try { $r = Invoke-WebRequest -Uri "http://127.0.0.1:9527/health" -UseBasicParsing -TimeoutSec 2; return ($r.StatusCode -ge 200 -and $r.StatusCode -lt 500) } catch { return $false }
}
Info "Starting knocode daemon..."
$daemonUp = Test-DaemonHealth
if ($daemonUp) {
  Ok "knocode daemon already running at http://127.0.0.1:9527 (status: running)"
} elseif (-not (Test-Path $installedDaemon)) {
  Warn "knocode-daemon.exe not found at $installedDaemon - build first (cargo build --release) then re-run installer or start manually"
} else {
  # Stale processes (holding old binary/port but not answering /health) - stop them before restart
  foreach ($procName in @("knocode-daemon", "knocode")) {
    Get-Process -Name $procName -ErrorAction SilentlyContinue | ForEach-Object {
      try { Stop-Process -Id $_.Id -Force -ErrorAction Stop; Info "  stopped stale $procName PID $($_.Id)" } catch {}
    }
  }
  $prevEA3 = $ErrorActionPreference; $ErrorActionPreference = "Continue"
  try {
    # WorkingDirectory: user .knocode home (repo-independent); scoping comes from per-request repository_path
    $daemonWorkDir = Join-Path $env:USERPROFILE ".knocode"
    New-Item -ItemType Directory -Force -Path $daemonWorkDir | Out-Null
    $daemonProc = Start-Process -FilePath $installedDaemon -WorkingDirectory $daemonWorkDir -WindowStyle Hidden -PassThru -ErrorAction Stop
    for ($i = 0; $i -lt 40; $i++) {
      Start-Sleep -Milliseconds 500
      if ($daemonProc.HasExited) { break }
      if (Test-DaemonHealth) { $daemonUp = $true; break }
    }
    if ($daemonUp) { Ok "knocode daemon RUNNING (PID $($daemonProc.Id), http://127.0.0.1:9527, from $installedDaemon)" }
    elseif ($daemonProc.HasExited) { Warn "daemon exited immediately (exit code $($daemonProc.ExitCode)) - start manually: $installedDaemon (check .knocode\config.toml)" }
    else { Warn "daemon started (PID $($daemonProc.Id)) but /health not responding within 20s - verify: curl http://127.0.0.1:9527/metrics" }
  } catch { Warn "failed to start daemon: $_ - start manually: $installedDaemon" }
  $ErrorActionPreference = $prevEA3
}

Info "Done - daemon: $(if ($daemonUp) { 'RUNNING at http://127.0.0.1:9527' } else { 'NOT running (start: ' + $installedDaemon + ')' }) | agents: $(if ($agentSel.Count -gt 0) { $agentSel -join ', ' } else { 'none' }) | rtk: $rtkStatus | log verbosity: $verbosity ($envLogLevel; logs: ~/.knocode/logs/knocode.log) | knocode doctor"
Info "Docs: docs/*.md | knocode doctor"
