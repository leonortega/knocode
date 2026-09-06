#!/usr/bin/env bash
# Knocode end-user installer (Linux x64 / macOS arm64) - installs a prebuilt GitHub release
#
# Downloads the matching prebuilt archive from the latest GitHub Release (or pinned
# via --version) and installs knocode + knocode-daemon into ~/.knocode/bin, then
# ensures that directory is on PATH.
#
# Prerequisites are installed automatically when missing (unless --skip-prereqs):
#   - Git - required by the runtime (commit-mode repo watching).
#   - Python 3.11+ - required by the runtime.
#   - Node.js LTS - required only when agent integrations are selected.
#   - RTK (prebuilt from GitHub releases) - optional external tool, offered AFTER
#     agent selection and ONLY when agent integrations are selected (opt-in: asked
#     interactively, or force/skip with --with-rtk / --no-rtk). RTK's own per-agent
#     integrations are wired via `rtk init -g` for each selected agent.
#
# Agent integrations (OpenCode / Copilot) are optional and
# selected interactively. They use the integration bundles shipped inside the
# release archive - no npm registry needed.
#
# One-liner:
#   curl -fsSL https://leonortega.github.io/knocode/install.sh | bash
#
# Pinned version:
#   curl -fsSL https://leonortega.github.io/knocode/install.sh | bash -s -- --version 0.9.11
set -euo pipefail

REPO="leonortega/knocode"
VERSION=""
AGENTS=""
ALL_AGENTS=false
NO_AGENTS=false
SKIP_PREREQS=false

WITH_RTK=false
NO_RTK=false

for arg in "$@"; do case "$arg" in
  --version) VERSION="$2"; shift;;
  --version=*) VERSION="${arg#--version=}";;
  --agents) AGENTS="$2"; shift;;
  --agents=*) AGENTS="${arg#--agents=}";;
  --all-agents) ALL_AGENTS=true;;
  --no-agents) NO_AGENTS=true;;
  --with-rtk) WITH_RTK=true;;
  --no-rtk) NO_RTK=true;;
  --skip-prereqs) SKIP_PREREQS=true;;
  -h|--help) echo "Usage: $0 [--version X.Y.Z] [--agents a,b,c|--all-agents|--no-agents] [--with-rtk|--no-rtk] [--skip-prereqs]"; exit 0;;
esac; done

info() { echo -e "\033[36m[knocode]\033[0m $*"; }
ok()   { echo -e "  \033[32m[OK]\033[0m $*"; }
warn() { echo -e "  \033[33m[WARN]\033[0m $*"; }
skip() { echo -e "  \033[90m[SKIP]\033[0m $*"; }
fail() { echo -e "  \033[31m[FAIL]\033[0m $*" >&2; exit 1; }

# ── Agent catalog & selection ─────────────────────────────────────────────
AGENT_CATALOG="opencode copilot"
select_agents() {
  if [ "$NO_AGENTS" = true ]; then echo ""; return; fi
  if [ -n "$AGENTS" ]; then
    local sel=""
    IFS=',' read -ra parts <<< "$AGENTS"
    for a in "${parts[@]}"; do
      a="$(echo "$a" | tr '[:upper:]' '[:lower:]' | xargs)"
      case " $AGENT_CATALOG " in *" $a "*) sel="$sel $a";; *) warn "unknown agent '$a' - valid: opencode, copilot";; esac
    done
    if [ -z "$sel" ]; then fail "no valid agents in --agents ('$AGENTS')"; fi
    echo "$sel"; return
  fi
  if [ "$ALL_AGENTS" = true ]; then echo "$AGENT_CATALOG"; return; fi
  # Interactive multi-select when stdin is a terminal; default to NONE otherwise
  if [ ! -t 0 ]; then
    info "non-interactive run - no agent integrations installed (use --agents opencode,copilot or --all-agents to change)"
    echo ""; return
  fi
  local sel=""
  for a in $AGENT_CATALOG; do
    printf "  Wire up %s? [y/N] " "$a"
    read -r r
    case "$r" in y|Y|yes|YES) sel="$sel $a";; *) skip "$a skipped";; esac
  done
  echo "$sel"
}

info "Knocode installer (prebuilt release)"
AGENT_SEL="$(select_agents)"
if [ -n "$AGENT_SEL" ]; then info "Agent integrations:$AGENT_SEL"; else info "Agent integrations: none"; fi

# ── RTK opt-in state (prompt/download happens AFTER agent wiring, section 8b) ─
# RTK is offered ONLY when agent integrations were selected (RTK without a wired
# agent has nothing to integrate with). --with-rtk forces, --no-rtk skips.
RTK_BIN="$HOME/.knocode/bin/rtk"
RTK_CMD=""
RTK_STATUS=""
if [ "$NO_RTK" = true ]; then
  RTK_STATUS="skipped (--no-rtk)"
elif [ -z "$AGENT_SEL" ]; then
  RTK_STATUS="skipped (no agent integrations selected)"
  if [ "$WITH_RTK" = true ]; then
    warn "--with-rtk was set but no agent integrations were selected - RTK not installed (re-run with --agents opencode,copilot)"
  fi
fi

# ── Architecture detection ────────────────────────────────────────────────
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m | tr '[:upper:]' '[:lower:]')"
case "$OS:$ARCH" in
  linux:x86_64)  ASSET_SUFFIX="x86_64-unknown-linux-gnu";;
  linux:aarch64) ASSET_SUFFIX="aarch64-unknown-linux-gnu";;
  darwin:arm64|darwin:aarch64) ASSET_SUFFIX="aarch64-apple-darwin";;
  darwin:x86_64) ASSET_SUFFIX="x86_64-apple-darwin";;
  *) fail "unsupported platform $OS/$ARCH - knocode releases are built for Linux x64, macOS arm64/x64";;
esac

# ── Resolve version ───────────────────────────────────────────────────────
if [ -n "$VERSION" ]; then
  TAG="$(echo "$VERSION" | sed 's/^v//')"
  TAG="v${TAG#v}"
else
  info "Resolving latest release..."
  TAG="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"//;s/".*//' || true)"
  if [ -z "$TAG" ]; then fail "could not resolve latest release from https://api.github.com/repos/$REPO/releases/latest"; fi
fi
VER="${TAG#v}"
if ! echo "$VER" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+'; then fail "invalid release tag '$TAG'"; fi
info "Installing knocode $VER"

# ── Stop running daemon/CLI ───────────────────────────────────────────────
for p in knocode-daemon knocode; do
  if pgrep -x "$p" >/dev/null 2>&1; then pkill -x "$p" 2>/dev/null || true; ok "stopped $p"; fi
done

# ── Download and extract ──────────────────────────────────────────────────
ASSET="knocode-${VER}-${ASSET_SUFFIX}.zip"
URL="https://github.com/$REPO/releases/download/$TAG/$ASSET"
TMP="$(mktemp -d 2>/dev/null || echo "$HOME/.cache/tmp/knocode_install")"
mkdir -p "$TMP"
intsDst=""

cleanup() { rm -rf "$TMP" 2>/dev/null || true; }
trap cleanup EXIT

info "Downloading $URL"
if ! curl -fsSL "$URL" -o "$TMP/$ASSET" 2>/dev/null; then
  fail "download failed: $URL"
fi

# Verify sha256 if sidecar exists (fail-open for older releases)
if curl -fsSL "$URL.sha256" -o "$TMP/$ASSET.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "$TMP/$ASSET.sha256" | tr '[:upper:]' '[:lower:]')"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$TMP/$ASSET" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$TMP/$ASSET" | awk '{print $1}')"
  else
    actual=""
  fi
  if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
    fail "checksum mismatch for $ASSET (expected $expected, got $actual)"
  fi
  if [ -n "$actual" ]; then ok "sha256 verified ($expected)"; fi
else
  warn "sha256 sidecar unavailable - skipping verification"
fi

mkdir -p "$TMP/extract"
unzip -qo "$TMP/$ASSET" -d "$TMP/extract" 2>/dev/null || fail "failed to extract $ASSET"

CLI_SRC="$(find "$TMP/extract" -name 'knocode' -type f 2>/dev/null | head -1)"
DAEMON_SRC="$(find "$TMP/extract" -name 'knocode-daemon' -type f 2>/dev/null | head -1)"
if [ -z "$CLI_SRC" ] || [ ! -f "$CLI_SRC" ]; then fail "knocode binary not found in $ASSET (broken release archive)"; fi

# ── Install binaries to ~/.knocode/bin ────────────────────────────────────
BIN_DIR="$HOME/.knocode/bin"
mkdir -p "$BIN_DIR"
INSTALLED_CLI="$BIN_DIR/knocode"
cp -f "$CLI_SRC" "$INSTALLED_CLI" && chmod +x "$INSTALLED_CLI"
ok "knocode $VER installed to $INSTALLED_CLI"

INSTALLED_DAEMON="$BIN_DIR/knocode-daemon"
if [ -n "$DAEMON_SRC" ] && [ -f "$DAEMON_SRC" ]; then
  cp -f "$DAEMON_SRC" "$INSTALLED_DAEMON" && chmod +x "$INSTALLED_DAEMON"
  ok "knocode-daemon installed to $INSTALLED_DAEMON"
else
  warn "knocode-daemon not found in $ASSET - daemon features unavailable"
fi

# Install bundled agent integration packages
INTS_SRC="$TMP/extract/integrations"
if [ -d "$INTS_SRC" ]; then
  intsDst="$HOME/.knocode/integrations"
  mkdir -p "$intsDst"
  cp -rf "$INTS_SRC/"* "$intsDst/" 2>/dev/null
  ok "agent integration bundles installed to $intsDst"
else
  warn "no bundled integrations in $ASSET - agent wiring will be unavailable"
fi

# Install the knocode agent skill (opencode — agent-native discovery)
SKILL_SRC="$TMP/extract/skills/knocode"
if [ -f "$SKILL_SRC/SKILL.md" ]; then
  OC_SKILL_DST="$HOME/.config/opencode/skills/knocode"
  mkdir -p "$(dirname "$OC_SKILL_DST")"
  cp -rf "$SKILL_SRC" "$OC_SKILL_DST"
  ok "knocode skill installed to $OC_SKILL_DST (opencode agent-native)"
else
  warn "knocode skill not found in $ASSET - skipping skill install"
fi

# ── Persist on PATH ───────────────────────────────────────────────────────
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) export PATH="$BIN_DIR:$PATH" ;; esac
for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
  if [ -f "$rc" ]; then
    grep -qs "KNOCODE_BIN_PATH" "$rc" || printf '\n# KNOCODE_BIN_PATH: knocode AI runtime CLI + daemon\nexport PATH="$HOME/.knocode/bin:$PATH"\n' >> "$rc" && ok "PATH entry ensured in $rc"
  fi
done

# ── Verify binaries ───────────────────────────────────────────────────────
info "Verifying installation..."
if "$INSTALLED_CLI" --version 2>/dev/null; then
  ok "installed to $INSTALLED_CLI"
else
  warn "knocode failed to run"
fi

# ── Prerequisites ─────────────────────────────────────────────────────────
if [ "$SKIP_PREREQS" = true ]; then
  info "Skipping prerequisite installs (--skip-prereqs)"
else
  # Git
  if ! command -v git >/dev/null 2>&1; then
    info "Installing git..."
    if command -v apt-get >/dev/null 2>&1; then sudo apt-get update -qq && sudo apt-get install -y git 2>/dev/null && ok "$(git --version)" || warn "git install failed - install manually: https://git-scm.com"
    elif command -v brew >/dev/null 2>&1; then brew install git 2>/dev/null && ok "$(git --version)" || warn "git install failed"
    elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y git 2>/dev/null && ok "$(git --version)" || warn "git install failed"
    elif command -v pacman >/dev/null 2>&1; then sudo pacman -S --noconfirm git 2>/dev/null && ok "$(git --version)" || warn "git install failed"
    else warn "git not found - install manually: https://git-scm.com"; fi
  else ok "$(git --version)"; fi

  # Python 3.11+
  if ! command -v python3 >/dev/null 2>&1 && ! command -v python >/dev/null 2>&1; then
    info "python3 not found - attempting install..."
    if command -v apt-get >/dev/null 2>&1; then sudo apt-get update -qq && sudo apt-get install -y python3 python3-pip 2>/dev/null && ok "python3 $(python3 --version)" || warn "python3 install failed - install manually: https://www.python.org/downloads/"
    elif command -v brew >/dev/null 2>&1; then brew install python@3.13 2>/dev/null && ok "python3 $(python3 --version)" || warn "python3 install failed"
    elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y python3 python3-pip 2>/dev/null && ok "python3 $(python3 --version)" || warn "python3 install failed"
    elif command -v pacman >/dev/null 2>&1; then sudo pacman -S --noconfirm python python-pip 2>/dev/null && ok "python3 $(python3 --version)" || warn "python3 install failed"
    else warn "python3 not found - install manually: https://www.python.org/downloads/"; fi
  else
    PYCMD="$(command -v python3 2>/dev/null || command -v python 2>/dev/null)"
    PYVER="$($PYCMD --version 2>&1 | grep -oE '[0-9]+\.[0-9]+')"
    PYMAJOR="${PYVER%%.*}"
    if [ "$PYMAJOR" -ge 3 ]; then ok "python $PYVER"
    else warn "python found but version < 3.11 - install manually: https://www.python.org/downloads/"; fi
  fi

  # Node.js LTS (required for agent integrations)
  if ! command -v node >/dev/null 2>&1; then
    info "node not found - attempting install..."
    if command -v apt-get >/dev/null 2>&1; then sudo apt-get update -qq && sudo apt-get install -y nodejs npm 2>/dev/null && ok "node $(node --version)" || warn "node install failed - install manually: https://nodejs.org"
    elif command -v brew >/dev/null 2>&1; then brew install node 2>/dev/null && ok "node $(node --version)" || warn "node install failed"
    elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y nodejs npm 2>/dev/null && ok "node $(node --version)" || warn "node install failed"
    elif command -v pacman >/dev/null 2>&1; then sudo pacman -S --noconfirm nodejs npm 2>/dev/null && ok "node $(node --version)" || warn "node install failed"
    else warn "no package manager for node - install manually: https://nodejs.org"; fi
  else ok "node $(node --version)"; fi
fi

# ── Agent integrations ────────────────────────────────────────────────────
if [ -n "$AGENT_SEL" ]; then
  info "Wiring agent integrations:$AGENT_SEL"

  if [ -z "$intsDst" ] || [ ! -d "$intsDst" ]; then
    warn "integration bundles not installed - agent wiring skipped"
  else
    # Node.js check (required for all agent integrations)
    if ! command -v node >/dev/null 2>&1; then
      if [ "$SKIP_PREREQS" = true ]; then
        warn "Node.js is required for agent integrations and --skip-prereqs was set - agents skipped"
        AGENT_SEL=""
      else
        warn "Node.js not found - agent integrations skipped (install Node from https://nodejs.org)"
        AGENT_SEL=""
      fi
    fi

    # --- OpenCode ---
    if echo "$AGENT_SEL" | grep -qw opencode; then
      OC_GLOBAL="$HOME/.config/opencode"
      PLUGIN_SRC="$intsDst/opencode-knocode"
      if [ -f "$PLUGIN_SRC/dist/index.js" ]; then
        mkdir -p "$OC_GLOBAL/node_modules"
        cp -rf "$PLUGIN_SRC" "$OC_GLOBAL/node_modules/"
        OC_CFG="$OC_GLOBAL/opencode.jsonc"
        if [ ! -f "$OC_CFG" ] || ! grep -q "opencode-knocode" "$OC_CFG" 2>/dev/null; then
          cat > "$OC_CFG" <<'OCEOF'
{
    "$schema": "https://opencode.ai/config.json",
    "plugin": ["opencode-knocode"]
}
OCEOF
        fi
        ok "opencode plugin installed (bundled opencode-knocode)"
        info "Restart opencode to load the plugin (daemon http://127.0.0.1:9527)"
      else
        warn "bundled opencode-knocode has no dist/index.js"
      fi
    fi

    # --- Copilot (VS Code): NO user-level MCP registration ---
    # The knocode MCP is internal to the Copilot Agent Plugin (plugin mcp.json ->
    # ${PLUGIN_ROOT}/servers/knocode-mcp.mjs) and is never exposed globally.
    # Clean up any knocode entry left in VS Code's user mcp.json by previous installs.
    if echo "$AGENT_SEL" | grep -qw copilot; then
      CODE_USER_DIR="$HOME/.config/Code/User"; VSCODE_MCP="$CODE_USER_DIR/mcp.json"
      if [ -f "$VSCODE_MCP" ] && command -v node >/dev/null 2>&1; then
        VSCODE_MCP_PATH="$VSCODE_MCP" node -e "const fs=require('fs');const p=process.env.VSCODE_MCP_PATH;let j={};try{j=JSON.parse(fs.readFileSync(p,'utf8'))}catch(e){};if(j.servers&&j.servers.knocode){delete j.servers.knocode;fs.writeFileSync(p,JSON.stringify(j,null,2));console.log('removed')}" 2>/dev/null | grep -q removed && ok "removed legacy knocode MCP entry from $VSCODE_MCP (MCP is plugin-internal only)" || true
      fi

      # --- Copilot Agent Plugin (hooks: SessionStart/PreToolUse/PostToolUse) ---
      # Deploy bundled plugin to ~/.knocode/copilot-plugin (repo-independent, survives
      # repo moves). The knocode MCP inside it (servers/knocode-mcp.mjs via the plugin's
      # own mcp.json) is internal to the plugin and never exposed globally.
      CP_PLUGIN_SRC="$intsDst/knocode-copilot-plugin"
      CP_PLUGIN_DST="$HOME/.knocode/copilot-plugin"
      if [ -f "$CP_PLUGIN_SRC/plugin.json" ]; then
        rm -rf "$CP_PLUGIN_DST" 2>/dev/null || true
        mkdir -p "$CP_PLUGIN_DST"
        cp -r "$CP_PLUGIN_SRC/." "$CP_PLUGIN_DST/" 2>/dev/null && ok "Copilot Agent Plugin deployed to $CP_PLUGIN_DST (hooks + MCP)" || warn "failed to deploy Copilot Agent Plugin to $CP_PLUGIN_DST"
      else
        warn "bundled knocode-copilot-plugin not found - skipping Agent Plugin deploy"
      fi

      # --- Copilot hooks (user-level ~/.copilot/hooks) ---
      # VS Code/Copilot does NOT discover agent plugins from ~/.knocode — the bundle
      # at $CP_PLUGIN_DST is only the hook-script home. Registration happens by writing
      # a hooks file into ~/.copilot/hooks/ (the same mechanism RTK uses), with an
      # absolute script path.
      if [ -f "$CP_PLUGIN_DST/scripts/knocode-hook.mjs" ]; then
        HOOK_SCRIPT="$CP_PLUGIN_DST/scripts/knocode-hook.mjs"
        mkdir -p "$HOME/.copilot/hooks"
        cat > "$HOME/.copilot/hooks/knocode-context.json" <<EOF
{
  "version": 1,
  "hooks": {
    "SessionStart": [
      {
        "type": "command",
        "command": "node \"$HOOK_SCRIPT\" session-start",
        "timeout": 15
      }
    ],
    "UserPromptSubmit": [
      {
        "type": "command",
        "command": "node \"$HOOK_SCRIPT\" user-prompt-submit",
        "timeout": 10
      }
    ]
  }
}
EOF
        ok "Copilot hooks registered at ~/.copilot/hooks/knocode-context.json (SessionStart + UserPromptSubmit)"
      else
        warn "knocode-hook.mjs not deployed - skipping Copilot hooks registration"
      fi

      # --- Knocode agent skill (Copilot global skills folder: ~/.copilot/skills) ---
      CP_SKILL_SRC="$TMP/extract/skills/knocode"
      if [ -f "$CP_SKILL_SRC/SKILL.md" ]; then
        mkdir -p "$HOME/.copilot/skills" && cp -rf "$CP_SKILL_SRC" "$HOME/.copilot/skills/" 2>/dev/null && ok "knocode skill installed to $HOME/.copilot/skills/knocode (Copilot global skills)" || warn "knocode skill copy (Copilot) failed (source: $CP_SKILL_SRC)"
      else warn "knocode skill not found in release archive - skipping Copilot agent skill install"; fi
    fi

    if [ -n "$AGENT_SEL" ]; then
      info "Agent integrations wired:$AGENT_SEL"
    fi
  fi
fi

# ── RTK (optional external tool) - DEPENDS ON AGENT SELECTION ─────────────
# Offered AFTER agent wiring and ONLY when agent integrations were selected
# (RTK without a wired agent has nothing to integrate with). Opt-in: --with-rtk
# forces, --no-rtk skips, otherwise asked interactively (default No). RTK ships
# its own OpenCode/Copilot integrations - knocode only installs the binary and
# wires them via `rtk init -g` in the RTK agent wiring section (no reimplementation).
if [ -z "$RTK_STATUS" ]; then
  WANT_RTK=false
  if [ "$WITH_RTK" = true ]; then WANT_RTK=true; fi
  if [ "$WANT_RTK" = false ]; then
    if [ -t 0 ]; then
      printf "  Also install RTK for the selected agents (%s)? [y/N] " "$(echo $AGENT_SEL | tr ' ' ',')"
      read -r r || true
      case "$r" in y|Y|yes|YES) WANT_RTK=true;; esac
    else
      RTK_STATUS="skipped (non-interactive, use --with-rtk)"
    fi
  fi
  if [ "$WANT_RTK" = true ]; then
    # Identity probe: the REAL rtk-ai/rtk has an `init` subcommand; name-collision
    # binaries on crates.io (e.g. "Rust Type Kit") fail on it. Never trust a bare
    # `rtk` on PATH without this check.
    is_real_rtk() { "$1" init --help >/dev/null 2>&1; }
    if command -v rtk >/dev/null 2>&1 && is_real_rtk rtk; then
      RTK_CMD="rtk"
      ok "rtk $(rtk --version 2>/dev/null | head -1)"
    elif [ -f "$RTK_BIN" ] && is_real_rtk "$RTK_BIN"; then
      RTK_CMD="$RTK_BIN"
      ok "rtk binary at $RTK_BIN"
    else
      if command -v rtk >/dev/null 2>&1; then
        BAD_RTK="$(command -v rtk)"
        warn "'rtk' found on PATH but it is NOT rtk-ai/rtk (name collision, e.g. Rust Type Kit) - removing it so it cannot shadow the real RTK"
        case "$BAD_RTK" in
          *\.cargo*) cargo uninstall rtk >/dev/null 2>&1 || true ;;
        esac
        rm -f "$BAD_RTK" 2>/dev/null || true
        hash -r 2>/dev/null || true
        if [ -f "$BAD_RTK" ]; then
          warn "could not remove $BAD_RTK - delete it manually or 'rtk' will still resolve to the wrong binary"
        fi
      fi
      RTK_ARCH="$(uname -m | tr '[:upper:]' '[:lower:]')"
      case "$OS:$RTK_ARCH" in
        linux:x86_64|linux:amd64) RTK_ASSET="rtk-x86_64-unknown-linux-musl.tar.gz";;
        linux:aarch64|linux:arm64) RTK_ASSET="rtk-aarch64-unknown-linux-gnu.tar.gz";;
        darwin:x86_64) RTK_ASSET="rtk-x86_64-apple-darwin.tar.gz";;
        darwin:aarch64|darwin:arm64) RTK_ASSET="rtk-aarch64-apple-darwin.tar.gz";;
        *) RTK_ASSET="";;
      esac
      if [ -z "$RTK_ASSET" ]; then
        warn "rtk: unsupported platform ($OS/$RTK_ARCH) - install manually from https://github.com/rtk-ai/rtk/releases"
      else
        RTK_URL="https://github.com/rtk-ai/rtk/releases/latest/download/$RTK_ASSET"
        RTK_TMP="$(mktemp -d 2>/dev/null || echo "$HOME/.cache/tmp/rtk_dl")"
        mkdir -p "$RTK_TMP"
        info "  downloading rtk release ($RTK_ASSET)..."
        if curl -fsSL "$RTK_URL" -o "$RTK_TMP/$RTK_ASSET" 2>/dev/null; then
          tar -xzf "$RTK_TMP/$RTK_ASSET" -C "$RTK_TMP" 2>/dev/null
          RTK_SRC="$(find "$RTK_TMP" -name rtk -type f 2>/dev/null | head -1 || true)"
          if [ -n "$RTK_SRC" ] && [ -f "$RTK_SRC" ]; then
            mkdir -p "$(dirname "$RTK_BIN")"
            cp -f "$RTK_SRC" "$RTK_BIN" && chmod +x "$RTK_BIN"
            RTK_CMD="$RTK_BIN"
            ok "rtk installed to $RTK_BIN (from GitHub release)"
          else
            warn "rtk release archive did not contain the rtk binary"
          fi
        else
          warn "rtk download failed - install manually from https://github.com/rtk-ai/rtk/releases"
        fi
        rm -rf "$RTK_TMP" 2>/dev/null
      fi
    fi
    if [ -n "$RTK_CMD" ]; then RTK_STATUS="installed"; elif [ -z "$RTK_STATUS" ]; then RTK_STATUS="failed"; fi
  elif [ -z "$RTK_STATUS" ]; then
    RTK_STATUS="declined"
  fi
fi

# ── RTK agent wiring (external tool) ─────────────────────────────────────
# RTK ships its own OpenCode (--opencode) and Copilot (--copilot) integrations.
# For every agent the user selected, hand off to RTK's own `rtk init -g`.
# Fail-open: never blocks the knocode install.
if [ -n "$AGENT_SEL" ] && [ -n "$RTK_CMD" ]; then
  info "Wiring RTK integrations for selected agents (external tool)..."
  if ! command -v rg >/dev/null 2>&1; then
    warn "ripgrep (rg) not on PATH - some rtk filters need it (apt/dnf/brew install ripgrep)"
  fi
  n=0
  total=$(echo $AGENT_SEL | wc -w | tr -d ' ')
  for a in $AGENT_SEL; do
    n=$((n + 1))
    info "  [$n/$total] wiring rtk for $a (runs: rtk init -g --$a --auto-patch - usually takes a few seconds)..."
    # stdin closed + output shown: rtk never waits silently on the installer's stdin,
    # and the user sees progress instead of a frozen prompt if it needs time.
    rtk_out=$("$RTK_CMD" init -g "--$a" --auto-patch </dev/null 2>&1)
    if [ $? -eq 0 ]; then
      ok "rtk integration wired for $a (rtk init -g --$a)"
      echo "$rtk_out" | grep -v '^[[:space:]]*$' | head -3 | sed 's/^/    /'
      # PATCH: RTK's generated plugin probes with `which rtk`, which does not
      # exist on Windows — swap the probe to `rtk --version` (portable). Must
      # run after EVERY `rtk init --opencode` (RTK regenerates the file).
      if [ "$a" = "opencode" ]; then
        RTK_OC_PLUGIN="$HOME/.config/opencode/plugins/rtk.ts"
        if [ -f "$RTK_OC_PLUGIN" ] && grep -q '`which rtk`' "$RTK_OC_PLUGIN"; then
          sed -i.bak 's/`which rtk`/`rtk --version`/' "$RTK_OC_PLUGIN" && rm -f "$RTK_OC_PLUGIN.bak"
          info "  [PATCH] opencode plugin probe: which rtk -> rtk --version (Windows-safe)"
        fi
      fi
    else
      warn "rtk init failed for $a (exit $?) - run manually: rtk init -g --$a"
      echo "$rtk_out" | head -5 | sed 's/^/    /'
    fi
  done
  info "RTK wiring done."
fi

# ── Start daemon ──────────────────────────────────────────────────────────
daemon_health() { curl -s -o /dev/null -m 2 http://127.0.0.1:9527/health; }
DAEMON_UP=no
if command -v curl >/dev/null 2>&1 && daemon_health; then
  DAEMON_UP=yes
  ok "knocode daemon already running at http://127.0.0.1:9527"
elif [ ! -x "$INSTALLED_DAEMON" ]; then
  warn "knocode-daemon not found at $INSTALLED_DAEMON - start manually"
else
  # Stop stale processes
  pkill -f knocode-daemon >/dev/null 2>&1 || true
  info "Starting knocode daemon..."
  mkdir -p "$HOME/.knocode"
  (cd "$HOME/.knocode" && nohup "$INSTALLED_DAEMON" >/dev/null 2>&1 &)
  if command -v curl >/dev/null 2>&1; then
    for _ in $(seq 1 40); do
      sleep 0.5
      if daemon_health; then DAEMON_UP=yes; break; fi
    done
  else
    sleep 3
    if pgrep -f knocode-daemon >/dev/null 2>&1; then DAEMON_UP=yes; fi
  fi
  if [ "$DAEMON_UP" = yes ]; then
    ok "knocode daemon RUNNING (http://127.0.0.1:9527)"
  else
    warn "daemon not responding on :9527 within 20s - start manually: $INSTALLED_DAEMON"
  fi
fi

info "Done - daemon: $(if [ "$DAEMON_UP" = yes ]; then echo 'RUNNING at http://127.0.0.1:9527'; else echo "NOT running (start: $INSTALLED_DAEMON)"; fi) | agents: $(if [ -n "$AGENT_SEL" ]; then echo "$AGENT_SEL"; else echo none; fi) | rtk: ${RTK_STATUS:-unknown}"
info "Next steps: open a new terminal, run 'knocode init' inside a project."
info "Docs: https://github.com/$REPO#readme"
