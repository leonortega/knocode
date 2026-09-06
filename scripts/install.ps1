#Requires -Version 5.1
<#
.SYNOPSIS
  Knocode installer v0.9.11 minimal (Windows PowerShell 5.1)
  Installs minimal v1 stack + uses prebuilt knocode (no compile/test). Idempotent - re-run to update.

.DESCRIPTION
  Minimal v1: Node >=20, Python+pip, Git, SQLite(bundled), tree-sitter/ripgrep/tantivy/tiktoken embedded,
         RTK (optional) - no Rust needed (prebuilt binaries; compile via scripts/compile.*)
  Agent integrations: OpenCode, Copilot (VS Code) - select one or more at install
  (default: all). Prebuilt: target/release/knocode.exe + knocode-daemon.exe are used directly.

.PARAMETER SkipBuild
  Deprecated - build is always skipped (prebuilt binary at target/release/knocode.exe is used). Kept for compat.

.PARAMETER Agents
  Comma-separated agent list to wire, e.g. "-Agents opencode". Valid: opencode, copilot.

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

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -Agents opencode
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -AllAgents
  powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -NoAgents
#>
param([switch]$SkipBuild, [string]$Agents = "", [switch]$AllAgents, [switch]$NoAgents, [switch]$WithRtk, [switch]$NoRtk, [switch]$SkipPrereqs)

$ErrorActionPreference = "Stop"
# Always English in scripts (avoid localized ShouldProcess/WhatIf)
try { [System.Threading.Thread]::CurrentThread.CurrentUICulture = [System.Globalization.CultureInfo]::GetCultureInfo('en-US'); [System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::GetCultureInfo('en-US') } catch {}
$Root = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $Root

function Test-Cmd($cmd) { $null -ne (Get-Command $cmd -ErrorAction SilentlyContinue) }
function Info($m) { Write-Host "[knocode] $m" -ForegroundColor Cyan }
function Ok($m) { Write-Host "  [OK] $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkGray }
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
$AgentCatalog = @("opencode", "copilot")

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
# CARGO_TARGET_DIR or [build] target is set in .cargo/config.toml.
# Detect via cargo metadata and copy binaries to repo-local target/release/.
if (-not (Test-Path $prebuilt)) {
  try {
    $metaJson = & cargo metadata --no-deps --format-version 1 2>$null | Out-String
    if ($LASTEXITCODE -eq 0 -and $metaJson) {
      $cargoTargetDir = ($metaJson | ConvertFrom-Json).target_directory
      if ($cargoTargetDir -and (Test-Path $cargoTargetDir)) {
        $cargoReleaseDir = Join-Path $cargoTargetDir "release"
        $srcKnocode = Join-Path $cargoReleaseDir "knocode.exe"
        $srcDaemon = Join-Path $cargoReleaseDir "knocode-daemon.exe"
        if (Test-Path $srcKnocode) {
          New-Item -ItemType Directory -Force -Path (Split-Path $prebuilt) | Out-Null
          Copy-Item -LiteralPath $srcKnocode -Destination $prebuilt -Force
          if (Test-Path $srcDaemon) { Copy-Item -LiteralPath $srcDaemon -Destination $prebuiltDaemon -Force }
          Info "Copied binaries from cargo target dir ($cargoReleaseDir) -> target/release/"
        }
      }
    }
  } catch {}
}
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
    $opencodeJsonc = @"
{
    "`$schema": "https://opencode.ai/config.json",
    "plugin": ["opencode-knocode"]
}
"@
    try { Set-Content -LiteralPath $ocGlobalCfg -Value $opencodeJsonc -Encoding UTF8; Ok "opencode plugin at $ocGlobalCfg" } catch { Warn "failed to write $ocGlobalCfg : $_" }
    # Remove legacy paths
    $globalPlugin = "$env:USERPROFILE\.config\opencode\plugins\knocode.ts"
    if (Test-Path $globalPlugin) { try { Remove-Item -LiteralPath $globalPlugin -Force } catch {} }
    $localPlugin = Join-Path $Root ".opencode\plugins\knocode.ts"
    if (Test-Path $localPlugin) { try { Remove-Item -LiteralPath $localPlugin -Force } catch {} }
    foreach ($f in @((Join-Path $Root ".opencode\opencode.jsonc"), (Join-Path $Root ".opencode\opencode.json"), (Join-Path $Root ".opencode\package.json"), (Join-Path $Root ".opencode\package-lock.json"))) {
      if (Test-Path $f) { try { Remove-Item -LiteralPath $f -Force } catch {} }
    }
    # Ensure npm plugin is built + installed into the global opencode config
    $pluginDir = Join-Path $Root "packages\opencode-knocode"
    $pluginDist = Join-Path $pluginDir "dist\index.js"
    if (Test-Path $pluginDir) {
      if (-not (Test-Path $pluginDist)) {
        if (Test-Cmd npm) {
          Info "Building opencode-knocode npm package..."
          Push-Location $pluginDir
          try {
            & npm install --silent 2>&1 | Out-Null
            & npm run build --silent 2>&1 | Out-Null
            if (Test-Path $pluginDist) { Ok "opencode-knocode built to packages/opencode-knocode/dist" } else { Warn "opencode-knocode build failed - run: cd packages/opencode-knocode; npm install; npm run build" }
          } catch { Warn "opencode-knocode build failed: $_" }
          Pop-Location
        } else { Warn "npm not found - cannot build opencode-knocode (install Node.js 18+)" }
      } else { Ok "opencode-knocode dist at packages/opencode-knocode/dist/index.js" }
      if (Test-Cmd npm) {
        $pkgJson = Join-Path $ocGlobalDir "package.json"
        $pluginFileRef = "file:" + ((Join-Path $Root "packages\opencode-knocode") -replace '\\','/')
        $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
        $pkgJsonContent = $null
        if (-not (Test-Path $pkgJson)) {
          $pkgJsonContent = (@{ dependencies = @{ "@opencode-ai/plugin" = "1.18.22"; "opencode-knocode" = $pluginFileRef } } | ConvertTo-Json -Depth 10)
          try { [IO.File]::WriteAllText($pkgJson, $pkgJsonContent, $utf8NoBom) } catch { Warn "failed to write $pkgJson : $_" }
        } else {
          try {
            $j = Get-Content -LiteralPath $pkgJson -Raw | ConvertFrom-Json
            if (-not $j.dependencies) { $j | Add-Member -NotePropertyName dependencies -NotePropertyValue @{} }
            # PS 5.1: dot-assignment of a NEW property on PSCustomObject throws -
            # must use Add-Member (mirrors the @opencode-ai/plugin handling below).
            if ($j.dependencies.PSObject.Properties["opencode-knocode"]) {
              $j.dependencies."opencode-knocode" = $pluginFileRef
            } else {
              $j.dependencies | Add-Member -NotePropertyName "opencode-knocode" -NotePropertyValue $pluginFileRef
            }
            if (-not $j.dependencies.PSObject.Properties["@opencode-ai/plugin"]) { $j.dependencies | Add-Member -NotePropertyName "@opencode-ai/plugin" -NotePropertyValue "1.18.22" }
            [IO.File]::WriteAllText($pkgJson, ($j | ConvertTo-Json -Depth 10), $utf8NoBom)
          } catch { Warn "failed to update $pkgJson : $_" }
        }
        Push-Location $ocGlobalDir
        try {
          $npmOut = & npm install 2>&1 | Out-String
          if (Test-Path "node_modules\opencode-knocode\dist\index.js") { Ok "opencode-knocode plugin installed" } else { Warn "opencode-knocode npm install failed - npm output:"; $npmOut.TrimEnd() -split "`n" | Select-Object -Last 10 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray } }
        } catch { Warn "opencode-knocode npm install failed: $_" }
        Pop-Location
      }
    } else { Warn "packages/opencode-knocode not found - skipping npm plugin install" }
    # Knocode agent skill (opencode - agent-native discovery)
    $ocSkillSrc = Join-Path $Root ".knocode\skills\knocode"
    if (Test-Path (Join-Path $ocSkillSrc "SKILL.md")) {
      try {
        New-Item -ItemType Directory -Force -Path (Join-Path $ocGlobalDir "skills") | Out-Null
        Copy-Item -LiteralPath $ocSkillSrc -Destination (Join-Path $ocGlobalDir "skills\knocode") -Recurse -Force
        Ok "knocode skill installed to $env:USERPROFILE\.config\opencode\skills\knocode (opencode agent-native)"
      } catch { Warn "knocode skill copy failed: $_" }
    } else { Warn ".knocode\skills\knocode not found - skipping agent skill install" }
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
            $existing | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $vscodeMcp -Encoding UTF8
            Ok "removed legacy knocode MCP entry from $vscodeMcp (MCP is plugin-internal only)"
          }
        } catch { Skip "could not clean knocode entry from $vscodeMcp" }
      }
    } catch { Warn "failed to clean VS Code Copilot MCP config: $_" }

    # --- Copilot Agent Plugin (hooks: SessionStart/PreToolUse/PostToolUse) ---
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
    if (Test-Path (Join-Path $pluginDst "scripts\knocode-hook.mjs")) {
      try {
        $hookScript = (Join-Path $pluginDst "scripts\knocode-hook.mjs") -replace '\\', '/'
        $knocodeHooksJson = @"
{
  "version": 1,
  "hooks": {
    "SessionStart": [
      {
        "type": "command",
        "command": "node \"$hookScript\" session-start",
        "timeout": 15
      }
    ],
    "UserPromptSubmit": [
      {
        "type": "command",
        "command": "node \"$hookScript\" user-prompt-submit",
        "timeout": 10
      }
    ]
  }
}
"@
        $copilotHooksDir = Join-Path $env:USERPROFILE ".copilot\hooks"
        New-Item -ItemType Directory -Force -Path $copilotHooksDir | Out-Null
        $knocodeHooksFile = Join-Path $copilotHooksDir "knocode-context.json"
        [IO.File]::WriteAllText($knocodeHooksFile, $knocodeHooksJson, (New-Object System.Text.UTF8Encoding($false)))
        Ok "Copilot hooks registered at $knocodeHooksFile (SessionStart + UserPromptSubmit)"
      } catch { Warn "failed to write Copilot hooks file: $_" }
    } else { Warn "knocode-hook.mjs not deployed - skipping Copilot hooks registration" }

    # --- Knocode agent skill (Copilot global skills folder: ~/.copilot/skills) ---
    $cpSkillSrc = Join-Path $Root ".knocode\skills\knocode"
    if (Test-Path (Join-Path $cpSkillSrc "SKILL.md")) {
      try {
        $cpSkillDst = Join-Path $env:USERPROFILE ".copilot\skills"
        New-Item -ItemType Directory -Force -Path $cpSkillDst | Out-Null
        Copy-Item -LiteralPath $cpSkillSrc -Destination (Join-Path $cpSkillDst "knocode") -Recurse -Force
        Ok "knocode skill installed to $cpSkillDst\knocode (Copilot global skills)"
      } catch { Warn "knocode skill copy (Copilot) failed: $_" }
    } else { Warn ".knocode\skills\knocode not found - skipping Copilot agent skill install" }

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

# 3b. RTK agent wiring - RTK ships its own OpenCode (--opencode) and Copilot
#     (--copilot) integrations. For every agent the user selected, hand off to
#     RTK's own `rtk init -g`. Fail-open: never blocks the knocode install.
if ($agentSel.Count -gt 0 -and $rtkCmd) {
  Info "Wiring RTK integrations for selected agents (external tool)..."
  if (-not (Test-Cmd rg)) {
    Warn "ripgrep (rg) not on PATH - some rtk filters need it (winget install BurntSushi.ripgrep.MSVC)"
  }
  foreach ($a in $agentSel) {
    Info "  [$($agentSel.IndexOf($a) + 1)/$($agentSel.Count)] wiring rtk for $a (runs: rtk init -g --$a --auto-patch - usually takes a few seconds)..."
    $prevEA = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try {
      # stdin closed + output shown: rtk never waits silently on the installer's stdin,
      # and the user sees progress instead of a frozen prompt if it needs time.
      $out = & $rtkCmd init -g --$a --auto-patch 2>&1
      if ($LASTEXITCODE -eq 0) {
        Ok "rtk integration wired for $a (rtk init -g --$a)"
        $out | Where-Object { $_ -and $_.ToString().Trim() } | Select-Object -First 3 | ForEach-Object { Info "    $_" }
      }
      else { Warn "rtk init failed for $a (exit $LASTEXITCODE) - run manually: rtk init -g --$a"; $out | Select-Object -First 5 | ForEach-Object { Info "    $_" } }
    } catch { Warn "rtk init failed for $a : $_" }
    $ErrorActionPreference = $prevEA
  }

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
        Set-Content -LiteralPath $rtkPlugin -Value $content -NoNewline -Encoding UTF8
        Ok "PATCH: rtk.ts probe now uses 'rtk --version' (Windows-safe)"
      }
    } catch { Warn "PATCH of rtk.ts failed: $($_.Exception.Message)" }
  }

  Info "RTK wiring done."
}

# 4. Start daemon - knocode must be in RUNNING state after installation
# TASK-037: launch the daemon from ~\.knocode\bin (installed copy) so the runtime keeps
# working if the repo is moved/cleaned — NOT from target\release.
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

Info "Done - daemon: $(if ($daemonUp) { 'RUNNING at http://127.0.0.1:9527' } else { 'NOT running (start: ' + $installedDaemon + ')' }) | agents: $(if ($agentSel.Count -gt 0) { $agentSel -join ', ' } else { 'none' }) | rtk: $rtkStatus | knocode doctor"
Info "Docs: docs/*.md | knocode doctor"
