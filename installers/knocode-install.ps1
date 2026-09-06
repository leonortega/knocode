#Requires -Version 5.1
<#
.SYNOPSIS
  Knocode end-user installer (Windows x64) - installs a prebuilt GitHub release
  and, on demand, all prerequisites (Git, Node.js) it needs.

.DESCRIPTION
  Downloads knocode-<ver>-x86_64-pc-windows-msvc.zip from the matching GitHub
  Release (latest by default, or pinned via -Version) and installs
  knocode.exe + knocode-daemon.exe into %USERPROFILE%\.knocode\bin, then ensures
  that directory is on the USER PATH (idempotent).

  Prerequisites are installed automatically - nothing depends on the user:
    - Git for Windows (per-user, silent) when missing - required by the runtime
      (commit-mode repo watching).
    - Python 3.11+ (per-user) when missing - required by the runtime.
    - Node.js LTS (per-user zip, no admin) when missing - required only when
      agent integrations are selected.
    - RTK (prebuilt from GitHub releases) - optional external tool, offered AFTER
      agent selection and ONLY when agent integrations are selected (opt-in: asked
      interactively, or force/skip with -WithRtk / -NoRtk). RTK's own per-agent
      integrations are wired via `rtk init -g` for each selected agent.

  Agent integrations (OpenCode / Copilot) are optional and
  selected interactively. They use the integration bundles shipped inside the
  release zip (integrations/opencode-knocode, integrations/knocode-mcp,
  integrations/knocode-copilot-plugin) - no npm registry needed.
  Use -SkipPrereqs to disable auto-installs.

  Latest release (one-liner):
    powershell -ExecutionPolicy Bypass -c "irm https://leonortega.github.io/knocode/install.ps1 | iex"

  Pinned version (download the script and pass -Version):
    powershell -ExecutionPolicy Bypass -File knocode-install.ps1 -Version 0.9.11

.PARAMETER Version
  Release version to install, e.g. "0.9.11" (leading "v" is optional).
  Defaults to the latest GitHub release.

.PARAMETER Agents
  Comma-separated agents to wire after install, e.g. "-Agents opencode".
  Valid: opencode, copilot.

.PARAMETER AllAgents
  Wire all supported agents without prompting.

.PARAMETER NoAgents
  Skip agent integration wiring entirely (default for non-interactive runs).

.PARAMETER WithRtk
  Install and wire RTK without prompting.

.PARAMETER NoRtk
  Skip RTK entirely (binary download + per-agent wiring).

.PARAMETER SkipPrereqs
  Do not auto-install Git/Python/Node.js - only warn when missing.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File knocode-install.ps1
  powershell -ExecutionPolicy Bypass -File knocode-install.ps1 -Version 0.9.11 -Agents opencode
#>
param([string]$Version = "", [string]$Agents = "", [switch]$AllAgents, [switch]$NoAgents, [switch]$WithRtk, [switch]$NoRtk, [switch]$SkipPrereqs)

$ErrorActionPreference = "Stop"
$Repo = "leonortega/knocode"
$AgentCatalog = @("opencode", "copilot")

function Write-Step($m) { Write-Host "[knocode] $m" -ForegroundColor Cyan }
function Write-Ok($m) { Write-Host "  [OK] $m" -ForegroundColor Green }
function Write-Warn($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; throw $m }

function Add-ToUserPath($dir) {
  try {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $userPath) { $userPath = "" }
    $entries = $userPath -split ";" | Where-Object { $_ -ne "" }
    if ($entries -notcontains $dir) {
      [Environment]::SetEnvironmentVariable("Path", (($entries + $dir) -join ";"), "User")
    }
  } catch { Write-Warn "could not persist PATH for $dir : $($_.Exception.Message)" }
  if (($env:Path -split ";") -notcontains $dir) { $env:Path = "$dir;$env:Path" }
}

function Install-NodeIfMissing {
  Write-Step "Installing Node.js LTS (per-user, no admin)..."
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
      Write-Ok "Node.js $ver installed to $nodeDir"
    } finally { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
  } catch { Write-Warn "Node.js auto-install failed: $($_.Exception.Message)" }
}

function Install-GitIfMissing {
  Write-Step "Installing Git for Windows (per-user, silent)..."
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
    Write-Ok "Git installed (per-user, $($rel.tag_name))"
  } catch { Write-Warn "Git auto-install failed: $($_.Exception.Message)" }
}

function Install-PythonIfMissing {
  Write-Step "Installing Python 3.13 (per-user)..."
  try {
    if (Get-Command winget -ErrorAction SilentlyContinue) {
      winget install --id Python.Python.3.13 -e --accept-package-agreements --accept-source-agreements --silent 2>&1 | Out-Null
      $pyPaths = @("$env:LOCALAPPDATA\Programs\Python\Python313\python.exe", "$env:LOCALAPPDATA\Programs\Python\Python313\Scripts\python.exe", "C:\Python313\python.exe")
      foreach ($p in $pyPaths) { if (Test-Path $p) { $env:Path = "$(Split-Path $p -Parent);$(Split-Path $p -Parent)\Scripts;$env:Path"; break } }
    } else {
      $pyUrl = "https://www.python.org/ftp/python/3.13.2/python-3.13.2-amd64.exe"
      $pyInst = "$env:TEMP\python-3.13.2-amd64.exe"
      Invoke-WebRequest -Uri $pyUrl -OutFile $pyInst -UseBasicParsing
      & $pyInst /quiet InstallAllUsers=0 PrependPath=1 Include_test=0 2>&1 | Out-Null
      Start-Sleep -Seconds 5
      $env:Path = "$env:LOCALAPPDATA\Programs\Python\Python313\Scripts;$env:LOCALAPPDATA\Programs\Python\Python313;$env:Path"
    }
    if (Get-Command python -ErrorAction SilentlyContinue) { Write-Ok "python $((python --version 2>&1) -join ' ')" }
    else { Write-Warn "python install attempted but python not on PATH - install manually: https://www.python.org/downloads/ (check 'Add to PATH')" }
  } catch { Write-Warn "python auto-install failed: $($_.Exception.Message)" }
}

function Test-CommandVersion($cmd, $minMajor) {
  try {
    $v = & $cmd --version 2>&1 | Select-Object -First 1
    if ($v -match '(\d+)\.(\d+)') { return ([int]$Matches[1] -ge $minMajor) }
  } catch {}
  return $false
}

function Select-Agents {
  if ($NoAgents) { return @() }
  if ($Agents -ne "") {
    $sel = @()
    foreach ($a in ($Agents -split ",")) {
      $a = $a.Trim().ToLower()
      if ($AgentCatalog -contains $a) { $sel += $a } else { Write-Warn "unknown agent '$a' - valid: $($AgentCatalog -join ', ')" }
    }
    if ($sel.Count -eq 0) { Fail "no valid agents in -Agents ('$Agents')" }
    return ($sel | Select-Object -Unique)
  }
  if ($AllAgents) { return @($AgentCatalog) }

  # Interactive prompt: each agent defaults to No - pick the ones you want
  $interactive = $true
  try { if ([Console]::IsInputRedirected) { $interactive = $false } } catch { $interactive = $false }
  if (-not $interactive) {
    Write-Step "non-interactive run - no agent integrations installed (pass -Agents opencode or -AllAgents)"
    return @()
  }
  Write-Step "Which agent integrations should be installed?"
  $sel = @()
  foreach ($a in $AgentCatalog) {
    $r = Read-Host "  Wire up $a ? [y/N]"
    if ($r -match "^(y|yes)$") { $sel += $a } else { Write-Host "  [SKIP] $a" -ForegroundColor DarkGray }
  }
  return $sel
}

Write-Step "Knocode installer (prebuilt release)"

# 1. Resolve the release tag (default: latest from the GitHub API)
$tag = ""
if ($Version -ne "") {
  $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
}
else {
  Write-Step "Resolving latest release..."
  try {
    $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "knocode-installer" } -UseBasicParsing
    $tag = $rel.tag_name
  }
  catch { Fail "could not resolve latest release from https://api.github.com/repos/$Repo/releases/latest ($($_.Exception.Message))" }
}
$ver = $tag.TrimStart("v")
if ($ver -notmatch "^\d+\.\d+\.\d+") { Fail "invalid release tag '$tag'" }
Write-Step "Installing knocode $ver"

# 2. Architecture guard - releases are built for Windows x64 only
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -eq "x86") {
  # 32-bit process on 64-bit Windows (WOW64)
  if ($env:PROCESSOR_ARCHITEW6432 -eq "AMD64") { $arch = "AMD64" }
}
if ($arch -ne "AMD64") { Fail "unsupported architecture '$arch' - knocode releases are built for Windows x64 (AMD64) only" }

# 3. Stop running daemon/CLI up front - later steps REPLACE the binaries and a
#    locked exe would fail the copy.
foreach ($procName in @("knocode-daemon", "knocode")) {
  Get-Process -Name $procName -ErrorAction SilentlyContinue | ForEach-Object {
    try { Stop-Process -Id $_.Id -Force -ErrorAction Stop; Write-Step "stopped $procName PID $($_.Id)" } catch { }
  }
}

# 4. Download the release archive, extract it, and install into %USERPROFILE%\.knocode\bin
$asset = "knocode-$ver-x86_64-pc-windows-msvc.zip"
$url = "https://github.com/$Repo/releases/download/$tag/$asset"
Write-Step "Downloading $url"
$tmp = Join-Path $env:TEMP ("knocode_install_" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$intsDst = ""
try {
  try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 } catch { }
  $zip = Join-Path $tmp $asset
  Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
  if (-not (Test-Path -LiteralPath $zip)) { Fail "download failed: $url" }

  # Verify the archive against the published .sha256 sidecar. Fail-open: older
  # releases without a sidecar still install (transport is HTTPS/TLS).
  try {
    $shaContent = (Invoke-WebRequest -Uri "$url.sha256" -UseBasicParsing -TimeoutSec 30).Content
    # PS 5.1 returns byte[] for non-text content types (e.g. application/octet-stream
    # on the .sha256 sidecar) - decode to text or -split compares decimal byte codes.
    if ($shaContent -is [byte[]]) { $shaContent = [Text.Encoding]::UTF8.GetString($shaContent) }
    $expectedHash = (($shaContent -split "\s+")[0]).Trim().ToLower()
    $actualHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
    if ($actualHash -ne $expectedHash) { Fail "checksum mismatch for $asset (expected $expectedHash, got $actualHash)" }
    Write-Ok "sha256 verified ($expectedHash)"
  }
  catch { Write-Warn "sha256 sidecar unavailable - skipping verification: $($_.Exception.Message)" }

  $extract = Join-Path $tmp "x"
  Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
  $cliSrc = Get-ChildItem -LiteralPath $extract -Recurse -Filter "knocode.exe" | Select-Object -First 1
  if (-not $cliSrc) { Fail "knocode.exe not found in $asset (broken release archive)" }
  $daemonSrc = Get-ChildItem -LiteralPath $extract -Recurse -Filter "knocode-daemon.exe" | Select-Object -First 1

  # 5. Copy the binaries into place
  $binDir = Join-Path $env:USERPROFILE ".knocode\bin"
  New-Item -ItemType Directory -Force -Path $binDir | Out-Null
  $installedCli = Join-Path $binDir "knocode.exe"
  Copy-Item -LiteralPath $cliSrc.FullName -Destination $installedCli -Force
  Write-Ok "knocode.exe $ver installed to $installedCli"
  if ($daemonSrc) {
    Copy-Item -LiteralPath $daemonSrc.FullName -Destination (Join-Path $binDir "knocode-daemon.exe") -Force
    Write-Ok "knocode-daemon.exe installed to $binDir\knocode-daemon.exe"
  }
  else { Write-Warn "knocode-daemon.exe missing from $asset - daemon features unavailable" }

  # 5b. Install the bundled agent integration packages (opencode-knocode, knocode-mcp)
  $intsSrc = Join-Path $extract "integrations"
  if (Test-Path $intsSrc) {
    $intsDst = Join-Path $env:USERPROFILE ".knocode\integrations"
    New-Item -ItemType Directory -Force -Path $intsDst | Out-Null
    Copy-Item -Path (Join-Path $intsSrc "*") -Destination $intsDst -Recurse -Force
    Write-Ok "agent integration bundles installed to $intsDst"
  }
  else { Write-Warn "no bundled integrations in $asset - agent wiring will be unavailable" }

  # 5c. Install the knocode agent skill (opencode — agent-native discovery)
  $skillSrc = Join-Path $extract "skills\knocode"
  if (Test-Path (Join-Path $skillSrc "SKILL.md")) {
    $ocDir = Join-Path $env:USERPROFILE ".config\opencode"
    $skillDst = Join-Path $ocDir "skills\knocode"
    New-Item -ItemType Directory -Force -Path (Join-Path $ocDir "skills") | Out-Null
    Copy-Item -LiteralPath $skillSrc -Destination $skillDst -Recurse -Force
    Write-Ok "knocode skill installed to $skillDst (opencode agent-native)"
  }
  else { Write-Warn "knocode skill not found in $asset - skipping skill install" }
}
finally {
  Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# 6. Ensure $binDir is on the USER PATH (HKCU Environment) - append only when missing
$binDir = Join-Path $env:USERPROFILE ".knocode\bin"
$installedCli = Join-Path $binDir "knocode.exe"
Add-ToUserPath $binDir

# 7. Verify binaries
Write-Step "Verifying installation..."
try {
  & $installedCli --version
  Write-Ok "installed to $installedCli"
}
catch { Write-Warn "knocode.exe failed to run: $($_.Exception.Message)" }

# 8. Prerequisites - auto-install anything missing (unless -SkipPrereqs)
if ($SkipPrereqs) {
  Write-Step "Skipping prerequisite installs (-SkipPrereqs)"
}
else {
  # Git - required by the runtime
  if (-not (Get-Command git -ErrorAction SilentlyContinue)) { Install-GitIfMissing }
  else { Write-Ok "git $(git --version)" }

  # Python 3.11+ - required by the runtime
  if (-not (Get-Command python -ErrorAction SilentlyContinue) -and -not (Get-Command python3 -ErrorAction SilentlyContinue)) {
    Install-PythonIfMissing
  } elseif (Get-Command python3 -ErrorAction SilentlyContinue) {
    if (Test-CommandVersion python3 3) { Write-Ok "python3 $((python3 --version 2>&1) -join ' ')" }
    else { Write-Warn "python3 found but version < 3.11 - installing newer version..."; Install-PythonIfMissing }
  } else {
    if (Test-CommandVersion python 3) { Write-Ok "python $((python --version 2>&1) -join ' ')" }
    else { Write-Warn "python found but version < 3.11 - installing newer version..."; Install-PythonIfMissing }
  }
}

# =============================================================================
# 9. Agent integrations (OpenCode / Copilot) - optional
# =============================================================================
$agentSel = @(Select-Agents)
if ($agentSel.Count -eq 0) {
  Write-Step "No agent integrations selected."
  Write-Step "Re-run with -Agents opencode,copilot (or -AllAgents) to wire agent integrations later."
}
else {
  Write-Step "Wiring agent integrations: $($agentSel -join ', ')"
  if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    if ($SkipPrereqs) {
      Write-Warn "Node.js is required for agent integrations and -SkipPrereqs was set - agents skipped (install Node from https://nodejs.org)"
      $agentSel = @()
    } else {
      Install-NodeIfMissing
    }
  }
  if ($agentSel.Count -gt 0 -and -not (Get-Command node -ErrorAction SilentlyContinue)) {
    Write-Warn "Node.js still not available after auto-install - agent integrations skipped"
    $agentSel = @()
  }
  if ($agentSel.Count -gt 0) { Write-Ok "node $(node --version)" }

  if ($agentSel.Count -gt 0 -and ($intsDst -eq "" -or -not (Test-Path $intsDst))) {
    Write-Warn "integration bundles not installed - agent wiring skipped"
    $agentSel = @()
  }

  # --- OpenCode: copy bundled plugin into the opencode config node_modules ---
  if ($agentSel -contains "opencode") {
    try {
      $pluginSrc = Join-Path $intsDst "opencode-knocode"
      if (Test-Path (Join-Path $pluginSrc "dist\index.js")) {
        $ocDir = Join-Path $env:USERPROFILE ".config\opencode"
        New-Item -ItemType Directory -Force -Path (Join-Path $ocDir "node_modules") | Out-Null
        Copy-Item -Path $pluginSrc -Destination (Join-Path $ocDir "node_modules\opencode-knocode") -Recurse -Force
        $ocCfg = Join-Path $ocDir "opencode.jsonc"
        if (-not (Test-Path $ocCfg) -or -not ((Get-Content -LiteralPath $ocCfg -Raw -ErrorAction SilentlyContinue) -match "opencode-knocode")) {
          Set-Content -LiteralPath $ocCfg -Value "{`n  `"`$schema`": `"https://opencode.ai/config.json`",`n  `"plugin`": [`"opencode-knocode`"]`n}`n" -Encoding UTF8
        }
        Write-Ok "opencode plugin installed (bundled opencode-knocode)"
        Write-Step "Restart opencode to load the plugin (daemon http://127.0.0.1:9527)"
      }
      else { Write-Warn "bundled opencode-knocode has no dist/index.js" }
    }
    catch { Write-Warn "opencode wiring failed: $($_.Exception.Message)" }
  }

  # --- Copilot (VS Code): NO user-level MCP registration ---
  # The knocode MCP is internal to the Copilot Agent Plugin (plugin mcp.json ->
  # ${PLUGIN_ROOT}/servers/knocode-mcp.mjs) and is never exposed globally.
  # Clean up any knocode entry left in VS Code's user mcp.json by previous installs.
  if ($agentSel -contains "copilot") {
    try {
      $vscodeMcp = Join-Path $env:APPDATA "Code\User\mcp.json"
      if (Test-Path $vscodeMcp) {
        $j = Get-Content -LiteralPath $vscodeMcp -Raw | ConvertFrom-Json
        if ($j.servers -and $j.servers.knocode) {
          $j.servers.PSObject.Properties.Remove('knocode')
          $j | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $vscodeMcp -Encoding UTF8
          Write-Ok "removed legacy knocode MCP entry from $vscodeMcp (MCP is plugin-internal only)"
        }
      }
    }
    catch { Write-Warn "Copilot MCP cleanup failed: $($_.Exception.Message)" }

    # --- Copilot Agent Plugin (hooks: SessionStart/PreToolUse/PostToolUse) ---
    # Deploy bundled plugin to %USERPROFILE%\.knocode\copilot-plugin (repo-independent,
    # survives repo moves). The knocode MCP inside it (servers/knocode-mcp.mjs via the
    # plugin's own mcp.json) is internal to the plugin and never exposed globally.
    $cpPluginSrc = Join-Path $intsDst "knocode-copilot-plugin"
    $cpPluginDst = Join-Path $env:USERPROFILE ".knocode\copilot-plugin"
    if (Test-Path (Join-Path $cpPluginSrc "plugin.json")) {
      try {
        # Fresh copy (idempotent update): clear destination first
        if (Test-Path $cpPluginDst) { Remove-Item -LiteralPath $cpPluginDst -Recurse -Force -ErrorAction SilentlyContinue }
        New-Item -ItemType Directory -Force -Path $cpPluginDst | Out-Null
        Copy-Item -Path (Join-Path $cpPluginSrc "*") -Destination $cpPluginDst -Recurse -Force
        Write-Ok "Copilot Agent Plugin deployed to $cpPluginDst (hooks + MCP)"
      } catch { Write-Warn "failed to deploy Copilot Agent Plugin: $($_.Exception.Message)" }
    } else { Write-Warn "bundled knocode-copilot-plugin not found - skipping Agent Plugin deploy" }

    # --- Copilot hooks (user-level ~/.copilot/hooks) ---
    # VS Code/Copilot does NOT discover agent plugins from ~/.knocode — the bundle
    # deployed to $cpPluginDst is only the hook-script home. Registration happens by
    # writing a hooks file into ~/.copilot/hooks/ (the same mechanism RTK uses for
    # rtk-rewrite.json), with an absolute script path (forward slashes: JSON-safe).
    if (Test-Path (Join-Path $cpPluginDst "scripts\knocode-hook.mjs")) {
      try {
        $hookScript = (Join-Path $cpPluginDst "scripts\knocode-hook.mjs") -replace '\\', '/'
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
        Write-Ok "Copilot hooks registered at $knocodeHooksFile (SessionStart + UserPromptSubmit)"
      } catch { Write-Warn "failed to write Copilot hooks file: $($_.Exception.Message)" }
    } else { Write-Warn "knocode-hook.mjs not deployed - skipping Copilot hooks registration" }

    # --- Knocode agent skill (Copilot global skills folder: ~/.copilot/skills) ---
    $cpSkillSrc = Join-Path $extract "skills\knocode"
    if (Test-Path (Join-Path $cpSkillSrc "SKILL.md")) {
      try {
        $cpSkillDst = Join-Path $env:USERPROFILE ".copilot\skills"
        New-Item -ItemType Directory -Force -Path $cpSkillDst | Out-Null
        Copy-Item -LiteralPath $cpSkillSrc -Destination (Join-Path $cpSkillDst "knocode") -Recurse -Force
        Write-Ok "knocode skill installed to $cpSkillDst\knocode (Copilot global skills)"
      } catch { Write-Warn "knocode skill copy (Copilot) failed: $($_.Exception.Message)" }
    } else { Write-Warn "knocode skill not found in release archive - skipping Copilot agent skill install" }
  }

  if ($agentSel.Count -gt 0) { Write-Step "Agent integrations wired: $($agentSel -join ', ')" }
}

# =============================================================================
# 9a. RTK (optional external tool) - DEPENDS ON AGENT SELECTION
#     Offered AFTER agent selection and ONLY when agent integrations were
#     selected (RTK without a wired agent has nothing to integrate with).
#     Opt-in: -WithRtk forces, -NoRtk skips, otherwise asked interactively (default No).
#     RTK ships its own OpenCode/Copilot integrations - knocode only installs the binary
#     and wires them via `rtk init -g` in section 9b (no reimplementation).
# =============================================================================
$rtkBinPath = Join-Path $env:USERPROFILE ".knocode\bin\rtk.exe"
$rtkCmd = $null
$rtkStatus = ""
if ($NoRtk) {
  $rtkStatus = "skipped (-NoRtk)"
}
elseif ($agentSel.Count -eq 0) {
  $rtkStatus = "skipped (no agent integrations selected)"
  if ($WithRtk) { Write-Warn "-WithRtk was set but no agent integrations were selected - RTK not installed (re-run with -Agents opencode,copilot)" }
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
    if ((Get-Command rtk -ErrorAction SilentlyContinue) -and (Test-RealRtk "rtk")) {
      $rtkCmd = "rtk"
      Write-Ok "rtk $((rtk --version 2>&1 | Select-Object -First 1) -join ' ')"
    } elseif ((Test-Path $rtkBinPath) -and (Test-RealRtk $rtkBinPath)) {
      $env:Path = "$(Split-Path $rtkBinPath -Parent);$env:Path"
      $rtkCmd = $rtkBinPath
      Write-Ok "rtk binary at $rtkBinPath"
    } else {
      if (Get-Command rtk -ErrorAction SilentlyContinue) {
        $badRtk = (Get-Command rtk -ErrorAction SilentlyContinue).Source
        Write-Warn "'rtk' found on PATH but it is NOT rtk-ai/rtk (name collision, e.g. Rust Type Kit) - removing it so it cannot shadow the real RTK"
        if ($badRtk -like "*\.cargo\*") { cargo uninstall rtk 2>&1 | Out-Null }
        try { Remove-Item -LiteralPath $badRtk -Force -ErrorAction Stop } catch {}
        if (Test-Path $badRtk) { Write-Warn "could not remove $badRtk - delete it manually or 'rtk' will still resolve to the wrong binary" }
      }
      $rtkAsset = "rtk-x86_64-pc-windows-msvc.zip"
      $rtkUrl = "https://github.com/rtk-ai/rtk/releases/latest/download/$rtkAsset"
      $rtkTmp = Join-Path $env:TEMP "rtk_dl"
      try {
        New-Item -ItemType Directory -Force -Path (Split-Path $rtkBinPath -Parent) | Out-Null
        if (Test-Path $rtkTmp) { Remove-Item -LiteralPath $rtkTmp -Recurse -Force -ErrorAction SilentlyContinue }
        New-Item -ItemType Directory -Force -Path $rtkTmp | Out-Null
        $rtkZip = Join-Path $rtkTmp $rtkAsset
        Write-Step "  downloading rtk release ($rtkAsset)..."
        Invoke-WebRequest -Uri $rtkUrl -OutFile $rtkZip -UseBasicParsing
        $rtkExtract = Join-Path $rtkTmp "x"
        Expand-Archive -LiteralPath $rtkZip -DestinationPath $rtkExtract -Force
        $srcExe = Get-ChildItem -LiteralPath $rtkExtract -Recurse -Filter "rtk.exe" | Select-Object -First 1
        if ($srcExe) {
          Copy-Item -LiteralPath $srcExe.FullName -Destination $rtkBinPath -Force
          $env:Path = "$(Split-Path $rtkBinPath -Parent);$env:Path"
          $rtkCmd = $rtkBinPath
          Write-Ok "rtk installed to $rtkBinPath (from GitHub release)"
        } else { Write-Warn "rtk release archive did not contain rtk.exe" }
      } catch { Write-Warn "rtk download failed: $($_.Exception.Message) - install manually from https://github.com/rtk-ai/rtk/releases" }
      finally { if (Test-Path $rtkTmp) { Remove-Item -LiteralPath $rtkTmp -Recurse -Force -ErrorAction SilentlyContinue } }
    }
    if ($rtkCmd) { $rtkStatus = "installed" } elseif ($rtkStatus -eq "") { $rtkStatus = "failed" }
  }
  elseif ($rtkStatus -eq "") { $rtkStatus = "declined" }
}

# =============================================================================
# 9b. RTK agent wiring - RTK ships its own OpenCode (--opencode) and Copilot
#     (--copilot) integrations. For every agent the user selected, hand off to
#     RTK's own `rtk init -g`. Fail-open: never blocks the knocode install.
# =============================================================================
if ($agentSel.Count -gt 0 -and $rtkCmd) {
  Write-Step "Wiring RTK integrations for selected agents (external tool)..."
  if (-not (Get-Command rg -ErrorAction SilentlyContinue)) {
    Write-Warn "ripgrep (rg) not on PATH - some rtk filters need it (winget install BurntSushi.ripgrep.MSVC)"
  }
  foreach ($a in $agentSel) {
    Write-Step "  [$($agentSel.IndexOf($a) + 1)/$($agentSel.Count)] wiring rtk for $a (runs: rtk init -g --$a --auto-patch - usually takes a few seconds)..."
    $prevEA = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try {
      # stdin closed + output shown: rtk never waits silently on the installer's stdin,
      # and the user sees progress instead of a frozen prompt if it needs time.
      $out = & $rtkCmd init -g --$a --auto-patch 2>&1
      if ($LASTEXITCODE -eq 0) {
        Write-Ok "rtk integration wired for $a (rtk init -g --$a)"
        $out | Where-Object { $_ -and $_.ToString().Trim() } | Select-Object -First 3 | ForEach-Object { Write-Step "    $_" }
      }
      else { Write-Warn "rtk init failed for $a (exit $LASTEXITCODE) - run manually: rtk init -g --$a"; $out | Select-Object -First 5 | ForEach-Object { Write-Step "    $_" } }
    } catch { Write-Warn "rtk init failed for $a : $($_.Exception.Message)" }
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
        Write-Ok "PATCH: rtk.ts probe now uses 'rtk --version' (Windows-safe)"
      }
    } catch { Write-Warn "PATCH of rtk.ts failed: $($_.Exception.Message)" }
  }

  Write-Step "RTK wiring done."
}

# =============================================================================
# 10. Start daemon - knocode must be in RUNNING state after installation
# =============================================================================
$binDir = Join-Path $env:USERPROFILE ".knocode\bin"
$installedDaemon = Join-Path $binDir "knocode-daemon.exe"
function Test-DaemonHealth {
  try { $r = Invoke-WebRequest -Uri "http://127.0.0.1:9527/health" -UseBasicParsing -TimeoutSec 2; return ($r.StatusCode -ge 200 -and $r.StatusCode -lt 500) } catch { return $false }
}
$daemonUp = Test-DaemonHealth
if ($daemonUp) {
  Write-Ok "knocode daemon already running at http://127.0.0.1:9527"
} elseif (-not (Test-Path $installedDaemon)) {
  Write-Warn "knocode-daemon.exe not found at $installedDaemon - start manually"
} else {
  # Stop stale processes first
  foreach ($procName in @("knocode-daemon", "knocode")) {
    Get-Process -Name $procName -ErrorAction SilentlyContinue | ForEach-Object {
      try { Stop-Process -Id $_.Id -Force -ErrorAction Stop; Write-Step "  stopped stale $procName PID $($_.Id)" } catch { }
    }
  }
  $prevEA = $ErrorActionPreference; $ErrorActionPreference = "Continue"
  try {
    $daemonWorkDir = Join-Path $env:USERPROFILE ".knocode"
    New-Item -ItemType Directory -Force -Path $daemonWorkDir | Out-Null
    $daemonProc = Start-Process -FilePath $installedDaemon -WorkingDirectory $daemonWorkDir -WindowStyle Hidden -PassThru -ErrorAction Stop
    for ($i = 0; $i -lt 40; $i++) {
      Start-Sleep -Milliseconds 500
      if ($daemonProc.HasExited) { break }
      if (Test-DaemonHealth) { $daemonUp = $true; break }
    }
    if ($daemonUp) { Write-Ok "knocode daemon RUNNING (PID $($daemonProc.Id), http://127.0.0.1:9527)" }
    elseif ($daemonProc.HasExited) { Write-Warn "daemon exited immediately (exit code $($daemonProc.ExitCode)) - start manually: $installedDaemon" }
    else { Write-Warn "daemon started but /health not responding within 20s - verify: curl http://127.0.0.1:9527/metrics" }
  } catch { Write-Warn "failed to start daemon: $($_.Exception.Message) - start manually: $installedDaemon" }
  $ErrorActionPreference = $prevEA
}

Write-Step "Done - daemon: $(if ($daemonUp) { 'RUNNING at http://127.0.0.1:9527' } else { 'NOT running (start: ' + $installedDaemon + ')' }) | agents: $(if ($agentSel.Count -gt 0) { $agentSel -join ', ' } else { 'none' }) | rtk: $rtkStatus"
Write-Step "Next steps: open a new terminal, run 'knocode init' inside a project."
Write-Step "Docs: https://github.com/$Repo#readme"