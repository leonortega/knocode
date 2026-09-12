#!/usr/bin/env bash
# Knocode installer minimal (Unix: Linux/macOS, bash)
# Minimal v1: Git + SQLite(bundled)/tree-sitter/tantivy/tiktoken embedded + RTK optional (no Rust - prebuilt binaries; compile via scripts/compile.sh)
# Agent integrations are selectable: --agents opencode,copilot,claude,cursor,gemini,codex,cline | --all-agents | --no-agents
# Log verbosity: --log-verbosity 0|1|2 (0 quiet / 1 normal / 2 verbose: log every daemon call); asked interactively otherwise
# Idempotent. Usage: bash scripts/install.sh [--skip-build] [--agents a,b,c|--all-agents|--no-agents] [--log-verbosity 0|1|2] [--skip-prereqs]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SKIP_BUILD=false; AGENTS=""; ALL_AGENTS=false; NO_AGENTS=false; WITH_RTK=false; NO_RTK=false; SKIP_PREREQS=false; LOG_VERBOSITY=""
for arg in "$@"; do case "$arg" in
  --skip-build) SKIP_BUILD=true;;
  --agents) AGENTS="$2"; shift;;
  --agents=*) AGENTS="${arg#--agents=}";;
  --all-agents) ALL_AGENTS=true;;
  --no-agents) NO_AGENTS=true;;
  --with-rtk) WITH_RTK=true;;
  --no-rtk) NO_RTK=true;;
  --skip-prereqs) SKIP_PREREQS=true;;
  --log-verbosity) LOG_VERBOSITY="$2"; shift;;
  --log-verbosity=*) LOG_VERBOSITY="${arg#--log-verbosity=}";;
  -h|--help) echo "Usage: $0 [--skip-build] [--agents opencode,copilot,claude,cursor,gemini,codex,cline | --all-agents | --no-agents] [--log-verbosity 0|1|2] [--with-rtk|--no-rtk] [--skip-prereqs]"; exit 0;;
esac; done
info(){ echo -e "\033[36m[knocode]\033[0m $*"; } ; ok(){ echo -e "  \033[32m[OK]\033[0m $*"; } ; warn(){ echo -e "  \033[33m[WARN]\033[0m $*"; } ; skip(){ echo -e "  \033[90m[SKIP]\033[0m $*"; }

# --- Agent catalog & selection -------------------------------------------------
# opencode/copilot have bespoke integrations (plugin / hooks). claude, cursor,
# gemini, codex and cline share the universal MCP+skill wiring below.
AGENT_CATALOG="opencode copilot claude cursor gemini codex cline"
select_agents() {
  if [ "$NO_AGENTS" = true ]; then echo ""; return; fi
  if [ -n "$AGENTS" ]; then
    local sel=""
    IFS=',' read -ra parts <<< "$AGENTS"
    for a in "${parts[@]}"; do
      a="$(echo "$a" | tr '[:upper:]' '[:lower:]' | xargs)"
      case " $AGENT_CATALOG " in *" $a "*) sel="$sel $a";; *) warn "unknown agent '$a' - valid: $AGENT_CATALOG";; esac
    done
    if [ -z "$sel" ]; then echo "error: no valid agents in --agents ('$AGENTS')" >&2; exit 1; fi
    echo "$sel"; return
  fi
  if [ "$ALL_AGENTS" = true ]; then echo "$AGENT_CATALOG"; return; fi
  # Interactive checkbox when stdin is a terminal; default to ALL otherwise
  if [ ! -t 0 ]; then
    info "non-interactive run - installing agent integrations for ALL agents (use --agents opencode or --no-agents to change)"
    echo "$AGENT_CATALOG"; return
  fi
  info "Which agent integrations should be installed? (checkbox)"
  i=0
  for a in $AGENT_CATALOG; do i=$((i + 1)); echo "  [$i] $a"; done
  printf "  Enter numbers separated by commas (e.g. 1,3), 'all', or press Enter for none: "
  read -r r || true
  r="$(echo "$r" | tr '[:upper:]' '[:lower:]' | xargs)"
  case "$r" in
    ""|none) info "no agent integrations selected"; echo ""; return;;
    all) echo "$AGENT_CATALOG"; return;;
  esac
  local sel=""
  for tok in $(echo "$r" | tr ',' ' '); do    case "$tok" in
      ''|*[!0-9]*) warn "ignoring invalid selection '$tok'";;
      *)
        n=$((10#$tok)); count=0; found=0
        for a in $AGENT_CATALOG; do
          count=$((count + 1))
          if [ "$count" -eq "$n" ]; then sel="$sel $a"; found=1; fi
        done
        if [ "$found" -eq 0 ]; then warn "ignoring invalid selection '$tok'"; fi
        ;;
    esac
  done
  sel="$(echo $sel)"
  if [ -z "$sel" ]; then info "no agent integrations selected"; fi
  echo "$sel"
}

info "Knocode installer"
AGENT_SEL="$(select_agents)"
if [ -n "$AGENT_SEL" ]; then info "Agent integrations:$(echo "$AGENT_SEL")"; else info "Agent integrations: none"; fi

# --- Log verbosity selection (0 quiet / 1 normal / 2 verbose) --------------------
# Shared knob: KNOCODE_LOG_LEVEL feeds BOTH the daemon ([logging] level fallback /
# env override) and the agent plugins (0 = errors only, 1 = outcome lines, 2 = one
# line per daemon call). select_verbosity echoes the chosen 0|1|2.
select_verbosity() {
  case "$LOG_VERBOSITY" in
    0|1|2) echo "$LOG_VERBOSITY"; return;;
    "") ;; # fall through to interactive/default
    *) warn "invalid --log-verbosity '$LOG_VERBOSITY' - valid: 0, 1, 2"; LOG_VERBOSITY="";;
  esac
  if [ ! -t 0 ]; then
    echo "1"; return
  fi
  printf "  Log verbosity? [0] quiet (errors only) [1] normal [2] verbose (every daemon call) [1] "
  read -r r
  case "$r" in 0) echo 0;; 2) echo 2;; *) echo 1;; esac
}
VERBOSITY="$(select_verbosity)"
case "$VERBOSITY" in
  0) LOG_LEVEL_STR="error"; VERBOSITY_DESC="quiet (errors only)";;
  2) LOG_LEVEL_STR="debug"; VERBOSITY_DESC="verbose (every daemon call logged)";;
  *) LOG_LEVEL_STR="info";  VERBOSITY_DESC="normal"; VERBOSITY=1;;
esac
info "Log verbosity: $VERBOSITY ($VERBOSITY_DESC)"

# 0a. Stop any running daemon/CLI up front - later steps REPLACE binaries (~/.knocode/bin)
# and a locked exe would fail the copy. The fresh daemon is restarted at the end (step 4).
for p in knocode-daemon knocode; do
  if pgrep -x "$p" >/dev/null 2>&1; then pkill -x "$p" 2>/dev/null || true; ok "stopped $p"; fi
done

# No Rust needed: knocode ships prebuilt (target/release) and the installer does not compile.
# Source builds use scripts/compile.sh (or CI).
if ! command -v node >/dev/null 2>&1; then
  if [ "$SKIP_PREREQS" = true ]; then warn "node not found - install Node >=20 https://nodejs.org (or re-run without --skip-prereqs)"
  else
    info "Installing Node.js (LTS)..."
    if command -v apt-get >/dev/null 2>&1; then sudo apt-get update -qq && sudo apt-get install -y nodejs npm 2>/dev/null && ok "node $(node --version)" || warn "node apt install failed - install manually: https://nodejs.org"
    elif command -v brew >/dev/null 2>&1; then brew install node 2>/dev/null && ok "node $(node --version)" || warn "node brew install failed"
    elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y nodejs npm 2>/dev/null && ok "node $(node --version)" || warn "node dnf install failed"
    else warn "no package manager for node - install manually: https://nodejs.org"; fi
  fi
else ok "node $(node --version)"; fi
if ! command -v python3 >/dev/null 2>&1 && ! command -v python >/dev/null 2>&1; then
  info "python3 not found - attempting install..."
  if command -v apt-get >/dev/null 2>&1; then sudo apt-get update -qq && sudo apt-get install -y python3 python3-pip 2>/dev/null && ok "python3 $(python3 --version)" || warn "python3 apt install failed - install manually: https://www.python.org/downloads/"
  elif command -v brew >/dev/null 2>&1; then brew install python@3.13 2>/dev/null && ok "python3 $(python3 --version)" || warn "python3 brew install failed"
  elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y python3 python3-pip 2>/dev/null && ok "python3 $(python3 --version)" || warn "python3 dnf install failed"
  else warn "python3 not found - install Python 3.11+ https://www.python.org/downloads/"; fi
else command -v python3 >/dev/null 2>&1 && ok "python3 $(python3 --version)" || ok "python $(python --version)"; fi
if ! command -v git >/dev/null 2>&1; then
  if [ "$SKIP_PREREQS" = true ]; then echo "git not found"; exit 1; fi
  info "Installing git..."
  if command -v apt-get >/dev/null 2>&1; then sudo apt-get update -qq && sudo apt-get install -y git 2>/dev/null && ok "$(git --version)" || { echo "git install failed"; exit 1; }
  elif command -v brew >/dev/null 2>&1; then brew install git 2>/dev/null && ok "$(git --version)" || { echo "git install failed"; exit 1; }
  elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y git 2>/dev/null && ok "$(git --version)" || { echo "git install failed"; exit 1; }
  else echo "git not found - install manually: https://git-scm.com"; exit 1; fi
else ok "$(git --version)"; fi


# 1. Use prebuilt knocode (no compile/test - use repository binary)
if [ "$SKIP_BUILD" = true ]; then info "Skipping build check (--skip-build)"; fi
info "Checking prebuilt knocode..."
# Fallback: cargo may use a global target dir (e.g. ~/.cargo/target) when
# CARGO_TARGET_DIR or [build] target-dir is set in .cargo/config.toml.
# Detect via cargo metadata and sync binaries into repo-local target/release/
# (mirrors install.ps1): copy when missing, refresh when the cargo-built
# binary is newer than the repo-local copy.
CARGO_RELEASE_DIR=""
if command -v cargo >/dev/null 2>&1; then
  cdir="$(cargo metadata --no-deps --format-version 1 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' | head -n1)"
  if [ -n "$cdir" ]; then
    cdir="$(printf '%s' "$cdir" | sed 's#\\\\#/#g')"  # JSON \\ -> / (Windows-safe, Unix-clean)
    [ -d "$cdir/release" ] && CARGO_RELEASE_DIR="$cdir/release"
  fi
fi
sync_prebuilt() { # $1 = binary name (knocode | knocode-daemon)
  for ext in "" ".exe"; do
    src="$CARGO_RELEASE_DIR/$1$ext"; dest="$ROOT/target/release/$1$ext"
    [ -n "$CARGO_RELEASE_DIR" ] && [ -f "$src" ] || continue
    if [ ! -f "$dest" ]; then
      mkdir -p "$ROOT/target/release"
      cp -f "$src" "$dest" && info "Copied $1$ext from cargo target dir ($CARGO_RELEASE_DIR) -> target/release/"
    elif [ "$src" -nt "$dest" ]; then
      cp -f "$src" "$dest" && info "Refreshed stale $1$ext from cargo target dir ($CARGO_RELEASE_DIR) -> target/release/"
    fi
  done
}
sync_prebuilt knocode
sync_prebuilt knocode-daemon
if [ -f "$ROOT/target/release/knocode" ] || [ -f "$ROOT/target/release/knocode.exe" ]; then ok "knocode at target/release/knocode(.exe)"; else warn "knocode binary not found at target/release/knocode - build manually: cargo build --release"; echo "prebuilt knocode missing - expected at target/release/knocode" >&2; exit 1; fi
if [ -f "$ROOT/target/release/knocode-daemon" ] || [ -f "$ROOT/target/release/knocode-daemon.exe" ]; then ok "knocode-daemon at target/release/knocode-daemon(.exe)"; else warn "knocode-daemon not found at target/release/knocode-daemon"; fi

# 1b. TASK-037: ship binaries to ~/.knocode/bin + persist on PATH, so knocode keeps working
# from any directory/shell even if this repo checkout is moved or cleaned. Idempotent re-run.
BIN_DIR="$HOME/.knocode/bin"
mkdir -p "$BIN_DIR"
SRC_CLI="$ROOT/target/release/knocode";   [ -f "$SRC_CLI" ]   || SRC_CLI="$ROOT/target/release/knocode.exe"
SRC_DAEMON="$ROOT/target/release/knocode-daemon"; [ -f "$SRC_DAEMON" ] || SRC_DAEMON="$ROOT/target/release/knocode-daemon.exe"
INSTALLED_CLI="$BIN_DIR/knocode"
if [ -f "$SRC_CLI" ]; then cp -f "$SRC_CLI" "$INSTALLED_CLI" 2>/dev/null && chmod +x "$INSTALLED_CLI" && ok "knocode installed to $INSTALLED_CLI" || { warn "failed to copy knocode to $BIN_DIR"; INSTALLED_CLI="$SRC_CLI"; }
else warn "no knocode binary to install (expected $ROOT/target/release/knocode)"; INSTALLED_CLI="$SRC_CLI"; fi
INSTALLED_DAEMON="$BIN_DIR/knocode-daemon"
if [ -f "$SRC_DAEMON" ]; then cp -f "$SRC_DAEMON" "$INSTALLED_DAEMON" 2>/dev/null && chmod +x "$INSTALLED_DAEMON" && ok "knocode-daemon installed to $INSTALLED_DAEMON" || { warn "failed to copy knocode-daemon to $BIN_DIR"; INSTALLED_DAEMON="$SRC_DAEMON"; }
else INSTALLED_DAEMON="$SRC_DAEMON"; fi
# Persist on PATH: idempotent append to ~/.profile and ~/.bashrc with a marker comment
for rc in "$HOME/.profile" "$HOME/.bashrc"; do
  if [ -f "$rc" ]; then
    grep -qs "KNOCODE_BIN_PATH" "$rc" || printf '\n# KNOCODE_BIN_PATH: knocode AI runtime CLI + daemon\nexport PATH="$HOME/.knocode/bin:$PATH"\n' >> "$rc" && ok "PATH entry ensured in $rc"
  fi
done
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) export PATH="$BIN_DIR:$PATH" ;; esac

# 1c. Persist log verbosity (shared KNOCODE_LOG_LEVEL knob)
#     a) user env var (agents/plugins read it regardless of shell/profile),
#     b) [logging] level in the USER config (~/.config/knocode/config.toml) so the
#        daemon honors it even when started outside a shell that has the var.
#     Env wins over config for the daemon; config keeps it discoverable via `knocode doctor`.
USER_CFG="$HOME/.config/knocode/config.toml"
if [ -f "$USER_CFG" ] && command -v node >/dev/null 2>&1; then
  USER_CFG_PATH="$USER_CFG" LOG_LEVEL_STR="$LOG_LEVEL_STR" node -e '
    const fs = require("fs");
    const p = process.env.USER_CFG_PATH;
    let t = fs.readFileSync(p, "utf8");
    if (/^\s*level\s*=/m.test(t)) t = t.replace(/^(\s*level\s*=).*/m, "$1 \"" + process.env.LOG_LEVEL_STR + "\"");
    else if (/^\[logging\]/m.test(t)) t = t.replace(/^(\[logging\]\n)/m, "$1level = \"" + process.env.LOG_LEVEL_STR + "\"\n");
    else t += (t.endsWith("\n") ? "" : "\n") + "\n[logging]\nlevel = \"" + process.env.LOG_LEVEL_STR + "\"\n";
    fs.writeFileSync(p, t);
  ' && ok "user config [logging] level = $LOG_LEVEL_STR ($USER_CFG)" || warn "failed to update [logging] level in $USER_CFG"
else
  mkdir -p "$(dirname "$USER_CFG")"
  printf '[logging]
level = "%s"
file_path = "~/.knocode/logs/knocode.log"
max_size_mb = 100
retention_days = 7
' "$LOG_LEVEL_STR" > "$USER_CFG" && ok "user config written ($USER_CFG, logging.level = $LOG_LEVEL_STR)" || warn "failed to write $USER_CFG"
fi
case "$VERBOSITY" in
  0) export KNOCODE_LOG_LEVEL="error";;
  2) export KNOCODE_LOG_LEVEL="debug";;
  *) export KNOCODE_LOG_LEVEL="info";;
esac
# Persist the env var for future shells: profile exports on Unix, HKCU on Windows (install.ps1).
if [ "$VERBOSITY" != "1" ]; then
  for rc in "$HOME/.profile" "$HOME/.bashrc"; do
    if [ -f "$rc" ]; then
      if grep -qs "KNOCODE_LOG_LEVEL" "$rc"; then
        ok "KNOCODE_LOG_LEVEL already set in $rc"
      else
        printf '\n# KNOCODE_LOG_LEVEL: knocode log verbosity (0 quiet / 1 normal / 2 verbose = every daemon call)\nexport KNOCODE_LOG_LEVEL="%s"\n' "$KNOCODE_LOG_LEVEL" >> "$rc" && ok "KNOCODE_LOG_LEVEL=$KNOCODE_LOG_LEVEL added to $rc"
      fi
    fi
  done
fi

# 2. Verify installation (doctor)
# NOTE: `knocode init` / `knocode index` are NOT run here on purpose - they bootstrap the
# repository they run IN (per-repo .knocode/ + index), which is meaningless for the knocode
# source checkout itself. Run them inside each repo you want analyzed.
info "Verifying installation (doctor)..."
"$INSTALLED_CLI" doctor

# Merge our plugin URL into opencode.jsonc, preserving any other entries
# (e.g. RTK's plugin). Never overwrites user config: writes fresh only when the
# file is missing, merges when node is available to parse it, warns + skips
# otherwise (leaving user config untouched).
merge_opencode_plugin() { # $1=config path $2=plugin url
  if [ ! -f "$1" ]; then
    printf '{\n    "$schema": "https://opencode.ai/config.json",\n    "plugin": ["%s"]\n}\n' "$2" > "$1" 2>/dev/null && ok "opencode config written at $1" || warn "failed to write $1"
    return
  fi
  if grep -qF "$2" "$1" 2>/dev/null; then ok "opencode plugin already registered in $1"; return; fi
  if command -v node >/dev/null 2>&1; then
    OC_CFG="$1" OC_URL="$2" node -e '
const fs=require("fs");
const p=process.env.OC_CFG, url=process.env.OC_URL;
const raw=fs.readFileSync(p,"utf8");
let stripped=raw.replace(/\/\/.*$/gm,"").replace(/\/\*[\s\S]*?\*\//g,"");
stripped=stripped.replace(/,\s*([\}\]])/g,"$1");
const j=JSON.parse(stripped);
let plugins=[];
if(Array.isArray(j.plugin))plugins=j.plugin.slice();
else if(typeof j.plugin==="string")plugins=[j.plugin];
if(!plugins.includes(url))plugins.push(url);
j.plugin=plugins;
fs.writeFileSync(p,JSON.stringify(j,null,2));console.log("merged");' 2>/dev/null && ok "opencode plugin merged into $1 (existing entries kept)" || warn "could not merge opencode plugin into $1 (leaving user config untouched)"
  else
    warn "cannot merge opencode plugin into $1 without node (leaving user config untouched) - add manually: $2"
  fi
}

# =====================================================================================
# 3. Agent integrations (OpenCode / Copilot) - selected above
# =====================================================================================
# Shared MCP stdio bridge (packages/knocode-mcp: zero-dep single file). Deployed
# ALWAYS - even with no agents selected - so the manual-MCP hint below points at
# a real file; every agent MCP entry points at it.
MCP_SRC="$ROOT/packages/knocode-mcp/dist/index.js"
if [ ! -f "$MCP_SRC" ] && command -v npm >/dev/null 2>&1 && [ -d "$ROOT/packages/knocode-mcp" ]; then
  (cd "$ROOT/packages/knocode-mcp" && npm install --silent 2>/dev/null && npm run build --silent 2>/dev/null) || true
fi
MCP_DST_DIR="$HOME/.knocode/mcp-server"; MCP_DST="$MCP_DST_DIR/knocode-mcp.mjs"
HAVE_BRIDGE=false
if [ -f "$MCP_SRC" ]; then
  mkdir -p "$MCP_DST_DIR" && cp -f "$MCP_SRC" "$MCP_DST" 2>/dev/null && HAVE_BRIDGE=true && ok "shared MCP bridge at $MCP_DST" || warn "MCP bridge deploy failed"
else warn "packages/knocode-mcp dist not built - run: cd packages/knocode-mcp && npm install && npm run build (MCP entries skipped, skills still install)"; fi

# Manual-MCP hint: printed when no agent integrations were selected.
show_mcp_hint() {
  if $HAVE_BRIDGE; then
    info "No agent integrations selected - use knocode as a plain MCP server instead:"
    echo "  1. Keep the daemon running: open a new terminal, run 'knocode init' inside a project"
    echo "     (MCP at http://127.0.0.1:9527/mcp, tool: knocode_context)"
    echo "  2. Add this to your MCP client's config file, then restart the client:"
    echo "     { \"mcpServers\": { \"knocode\": { \"command\": \"node\", \"args\": [\"$MCP_DST\"] } } }"
    echo "  3. Requires Node.js. Re-run this installer and pick agents to wire one automatically."
  else
    warn "No agent integrations selected - and the MCP bridge is unavailable (see warning above). Re-run with --agents to wire an agent."
  fi
}
if [ -z "$AGENT_SEL" ]; then show_mcp_hint; fi
if [ -n "$AGENT_SEL" ]; then
  OC_GLOBAL="$HOME/.config/opencode"

  # --- OpenCode: global plugin + skill (~/.config/opencode) ---
  if echo "$AGENT_SEL" | grep -qw opencode; then
    OC_GLOBAL_CFG="$OC_GLOBAL/opencode.jsonc"
    info "Configuring opencode plugin (global ~/.config/opencode)..."
    mkdir -p "$OC_GLOBAL"
    # NOTE: file:// spec (not the bare npm name) — opencode-knocode is not
    # published to the npm registry, and a bare spec makes the opencode
    # loader fail at the install stage so the plugin never loads. The
    # file:// URL loads the local build directly (self-contained esbuild
    # bundle at packages/opencode-knocode/dist/index.js - no npm step needed).
    merge_opencode_plugin "$OC_GLOBAL_CFG" "file://$ROOT/packages/opencode-knocode"
    ok "opencode plugin GLOBAL at $OC_GLOBAL_CFG (plugin: file:// local build, MCPs used internally by daemon)"
    # Remove legacy global path plugin (now npm)
    GLOBAL_PLUGIN="$HOME/.config/opencode/plugins/knocode.ts"
    if [ -f "$GLOBAL_PLUGIN" ]; then rm -f "$GLOBAL_PLUGIN" 2>/dev/null && info "Removed legacy global path plugin knocode.ts" || true; fi
    LOCAL_PLUGIN="$ROOT/.opencode/plugins/knocode.ts"
    if [ -f "$LOCAL_PLUGIN" ]; then rm -f "$LOCAL_PLUGIN" 2>/dev/null && info "Removed legacy local path plugin .opencode/plugins/knocode.ts" || true; fi
    # Migrate: remove per-project opencode config/deps (plugin is global now)
    for f in "$ROOT/.opencode/opencode.jsonc" "$ROOT/.opencode/opencode.json" "$ROOT/.opencode/package.json" "$ROOT/.opencode/package-lock.json"; do
      if [ -f "$f" ]; then rm -f "$f" 2>/dev/null && info "Removed legacy project $(basename "$f") (MCPs/plugin are global now)" || true; fi
    done
    # dist/index.js is a self-contained esbuild bundle (see packages/opencode-knocode) -
    # no npm step at install time. The file:// URL above loads the local build directly.
    if [ -f "$ROOT/packages/opencode-knocode/dist/index.js" ]; then
      ok "opencode-knocode dist at packages/opencode-knocode/dist/index.js (self-contained bundle)"
    elif [ -d "$ROOT/packages/opencode-knocode" ]; then
      warn "opencode-knocode dist not built - run: cd packages/opencode-knocode && npm install && npm run build"
    else warn "packages/opencode-knocode not found - skipping plugin install"; fi
    # NOTE: no skill install for opencode — the plugin injects context
    # transparently, so a skill is unnecessary (legacy
    # ~/.config/opencode/skills/knocode is removed by the uninstaller).
    info "Restart opencode to load global plugin 'opencode-knocode' (hook: chat.message, daemon http://127.0.0.1:9527). Plugin loads in EVERY project (global ~/.config/opencode)."
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

    # --- Copilot hooks (user-level ~/.copilot/hooks) ---
    # VS Code/Copilot does NOT discover agent plugins from ~/.knocode — the bundle
    # at $pluginDst is only the hook-script home. Registration happens by writing a
    # hooks file into ~/.copilot/hooks/ (the same mechanism RTK uses), with an
    # absolute script path. UserPromptSubmit injects the context (fires every
    # turn, incl. tool-less answers); PreToolUse is the consume-once retry.
    if [ -f "$pluginDst/scripts/knocode-hook.mjs" ]; then
      HOOK_SCRIPT="$pluginDst/scripts/knocode-hook.mjs"
      mkdir -p "$HOME/.copilot/hooks"
      cat > "$HOME/.copilot/hooks/knocode-context.json" <<EOF
{
  "version": 1,
  "hooks": {
    "UserPromptSubmit": [
      {
        "type": "command",
        "command": "node \"$HOOK_SCRIPT\" user-prompt-submit",
        "timeout": 5
      }
    ],
    "PreToolUse": [
      {
        "type": "command",
        "command": "node \"$HOOK_SCRIPT\" pre-tool-use",
        "timeout": 15
      }
    ]
  }
}
EOF
      ok "Copilot hooks registered at ~/.copilot/hooks/knocode-context.json (UserPromptSubmit + PreToolUse)"
    else
      warn "knocode-hook.mjs not deployed - skipping Copilot hooks registration"
    fi

    # NOTE: no skill install for Copilot — context flows through the hooks,
    # so a skill is unnecessary (legacy ~/.copilot/skills/knocode is removed
    # by the uninstaller). Universal agents below cover MCP+skill consumers.
  fi

  # --- Copilot Agent Plugin (hooks: UserPromptSubmit/PreToolUse) ---
  # Deploy to ~/.knocode/copilot-plugin (repo-independent, survives repo moves).
  if echo "$AGENT_SEL" | grep -qw copilot; then
    PLUGIN_SRC="$ROOT/packages/knocode-copilot-plugin"
    PLUGIN_DST="$HOME/.knocode/copilot-plugin"
    if [ -f "$PLUGIN_SRC/plugin.json" ]; then
      rm -rf "$PLUGIN_DST" 2>/dev/null || true
      mkdir -p "$PLUGIN_DST"
      if cp -r "$PLUGIN_SRC/." "$PLUGIN_DST/" 2>/dev/null; then
        ok "Copilot Agent Plugin deployed to $PLUGIN_DST (hooks + MCP)"
      else
        warn "failed to deploy Copilot Agent Plugin to $PLUGIN_DST"
      fi
    else
      warn "packages/knocode-copilot-plugin not found - skipping Agent Plugin deploy"
    fi

    # --- @knocode chat participant extension (VSIX via `code` CLI) ---
    EXT_DIR="$ROOT/packages/vscode-copilot-knocode"
    if [ -f "$EXT_DIR/package.json" ]; then
      if command -v npm >/dev/null 2>&1 && [ ! -f "$EXT_DIR/dist/extension.js" ]; then
        info "Building vscode-copilot-knocode extension..."
        (cd "$EXT_DIR" && npm install --silent 2>/dev/null && npm run build --silent 2>/dev/null) || warn "vscode-copilot-knocode build failed"
      fi
      if [ -z "$(ls "$EXT_DIR"/*.vsix 2>/dev/null)" ] && command -v npm >/dev/null 2>&1; then
        (cd "$EXT_DIR" && npx --yes @vscode/vsce package --no-dependencies >/dev/null 2>&1) || true
      fi
      VSIX="$(ls "$EXT_DIR"/*.vsix 2>/dev/null | head -n 1 || true)"
      if [ -n "$VSIX" ]; then
        if command -v code >/dev/null 2>&1; then
          info "Installing @knocode VS Code extension (code --install-extension)..."
          if code --install-extension "$VSIX" --force >/dev/null 2>&1; then
            ok "@knocode extension installed from $(basename "$VSIX") - reload VS Code to activate"
          else
            warn "code CLI install failed - install manually: code --install-extension $VSIX"
          fi
        else
          warn "VS Code 'code' CLI not on PATH - install manually: code --install-extension $VSIX"
        fi
      else
        warn "vscode-copilot-knocode VSIX not built - run: cd packages/vscode-copilot-knocode && npx @vscode/vsce package"
      fi
    else
      warn "packages/vscode-copilot-knocode not found - skipping @knocode extension install"
    fi
  fi

  # --- Universal agents (MCP + skill): claude, cursor, gemini, codex, cline ---
  # Skill copy per agent global folder + `knocode` MCP server entry in the agent's
  # global config. Fail-open per agent: one agent's failure never blocks others.
  if echo "$AGENT_SEL" | grep -qwE "claude|cursor|gemini|codex|cline"; then
    info "Configuring universal agents (MCP + skill)..."
    # Bridge is pre-deployed above (section 3 header) - MCP_DST/HAVE_BRIDGE.

    uni_skill() { # $1=agent $2=destdir
      if [ -f "$ROOT/.knocode/skills-universal/knocode/SKILL.md" ]; then
        mkdir -p "$(dirname "$2")" && rm -rf "$2" 2>/dev/null; cp -rf "$ROOT/.knocode/skills-universal/knocode" "$2" 2>/dev/null && ok "$1 skill at $2" || warn "$1 skill copy failed"
      else warn ".knocode/skills-universal/knocode not found - skipping $1 skill"; fi
    }
    uni_mcp_json() { # $1=agent $2=config path (mcpServers.knocode merge, keys preserved)
      if ! $HAVE_BRIDGE; then skip "$1 MCP skipped (no bridge)"; return; fi
      if ! command -v node >/dev/null 2>&1; then warn "$1 MCP: node needed to write $2"; return; fi
      MCP_CFG="$2" MCP_JS="$MCP_DST" node -e "
const fs=require('fs');const p=process.env.MCP_CFG;
try{
  let j={};
  if(fs.existsSync(p)){const raw=fs.readFileSync(p,'utf8');
    if(raw.trim()){try{j=JSON.parse(raw);}catch{j=JSON.parse(raw.replace(/\/\/.*$/gm,'').replace(/\/\*[\s\S]*?\*\//g,'').replace(/,\s*([\}\]])/g,'\$1'));}}}
  if(!j.mcpServers||typeof j.mcpServers!=='object')j.mcpServers={};
  j.mcpServers.knocode={command:'node',args:[process.env.MCP_JS]};
  fs.mkdirSync(require('path').dirname(p),{recursive:true});
  fs.writeFileSync(p,JSON.stringify(j,null,2));console.log('merged');
}catch(e){console.error(e.message);process.exit(1);}
" 2>/dev/null && ok "$1 MCP registered in $2" || warn "$1 MCP config failed ($2)"
    }

    if echo "$AGENT_SEL" | grep -qw claude; then
      uni_skill claude "$HOME/.claude/skills/knocode"
      uni_mcp_json claude "$HOME/.claude.json"
      info "note: if 'claude mcp list' shows no servers, run: claude mcp add --scope user knocode -- node $MCP_DST"
    fi
    if echo "$AGENT_SEL" | grep -qw cursor; then
      uni_skill cursor "$HOME/.cursor/skills/knocode"
      uni_mcp_json cursor "$HOME/.cursor/mcp.json"
    fi
    if echo "$AGENT_SEL" | grep -qw gemini; then
      uni_skill gemini "$HOME/.gemini/skills/knocode"
      uni_mcp_json gemini "$HOME/.gemini/settings.json"
    fi
    if echo "$AGENT_SEL" | grep -qw codex; then
      uni_skill codex "$HOME/.knocode/skills/knocode"
      if $HAVE_BRIDGE; then
        CODEX_CFG="$HOME/.codex/config.toml"; mkdir -p "$(dirname "$CODEX_CFG")"
        touch "$CODEX_CFG" 2>/dev/null || true
        if ! grep -q "mcp_servers.knocode" "$CODEX_CFG" 2>/dev/null; then
          printf '\n[mcp_servers.knocode]\ncommand = "node"\nargs = ["%s"]\n' "$MCP_DST" >> "$CODEX_CFG" && ok "codex MCP registered in ~/.codex/config.toml" || warn "codex MCP config failed"
        else ok "codex MCP already present in ~/.codex/config.toml"; fi
        if ! grep -qF "$HOME/.knocode/skills/knocode" "$CODEX_CFG" 2>/dev/null; then
          printf '\n[[skills.config]]\npath = "%s"\nenabled = true\n' "$HOME/.knocode/skills/knocode" >> "$CODEX_CFG" && ok "codex skill registered in ~/.codex/config.toml" || warn "codex skill config failed"
        fi
      else skip "codex MCP skipped (no bridge)"; fi
    fi
    if echo "$AGENT_SEL" | grep -qw cline; then
      uni_skill cline "$HOME/.cline/skills/knocode"
      uni_mcp_json cline "$HOME/.cline/data/settings/cline_mcp_settings.json"
    fi
  fi
fi

# 3a. RTK (optional external tool) - DEPENDS ON AGENT SELECTION
#     Offered AFTER agent wiring and ONLY when agent integrations were selected
#     (RTK without a wired agent has nothing to integrate with). Opt-in: --with-rtk
#     forces, --no-rtk skips, otherwise asked interactively (default No). RTK ships
#     its own OpenCode/Copilot integrations - knocode only installs the binary and
#     wires them via `rtk init -g` in section 3b (no reimplementation).
RTK_BIN="$HOME/.knocode/bin/rtk"
RTK_CMD=""
RTK_STATUS=""
if [ "$NO_RTK" = true ]; then
  RTK_STATUS="skipped (--no-rtk)"
elif [ -z "$AGENT_SEL" ]; then
  RTK_STATUS="skipped (no agent integrations selected)"
  if [ "$WITH_RTK" = true ]; then
    warn "--with-rtk was set but no agent integrations were selected - RTK not installed (re-run with --agents opencode,copilot,claude,cursor,gemini,codex,cline)"
  fi
else
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
    if [ -f "$HOME/bin/rtk" ] && [ ! -f "$RTK_BIN" ]; then mkdir -p "$(dirname "$RTK_BIN")"; cp -f "$HOME/bin/rtk" "$RTK_BIN" 2>/dev/null && chmod +x "$RTK_BIN" 2>/dev/null && ok "migrated legacy ~/bin/rtk -> $RTK_BIN" || true; fi
    # Identity probe: the REAL rtk-ai/rtk has an `init` subcommand; name-collision
    # binaries on crates.io (e.g. "Rust Type Kit") fail on it. Never trust a bare
    # `rtk` on PATH without this check.
    is_real_rtk() { "$1" init --help >/dev/null 2>&1; }
    if command -v rtk >/dev/null 2>&1 && is_real_rtk rtk; then RTK_CMD="rtk"; ok "rtk $(rtk --version 2>/dev/null | head -1)"
    elif [ -f "$RTK_BIN" ] && is_real_rtk "$RTK_BIN"; then RTK_CMD="$RTK_BIN"; ok "rtk binary at $RTK_BIN"
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
      RTK_OS="$(uname -s 2>/dev/null | tr "[:upper:]" "[:lower:]")"; RTK_ARCH="$(uname -m 2>/dev/null | tr "[:upper:]" "[:lower:]")"
      case "$RTK_OS:$RTK_ARCH" in
        linux:x86_64|linux:amd64) RTK_ASSET="rtk-x86_64-unknown-linux-musl.tar.gz";;
        linux:aarch64|linux:arm64) RTK_ASSET="rtk-aarch64-unknown-linux-gnu.tar.gz";;
        darwin:x86_64) RTK_ASSET="rtk-x86_64-apple-darwin.tar.gz";;
        darwin:aarch64|darwin:arm64) RTK_ASSET="rtk-aarch64-apple-darwin.tar.gz";;
        *) RTK_ASSET="";;
      esac
      if [ -z "$RTK_ASSET" ]; then warn "rtk: unsupported platform ($RTK_OS/$RTK_ARCH) - install manually from https://github.com/rtk-ai/rtk/releases"
      else
        RTK_URL="https://github.com/rtk-ai/rtk/releases/latest/download/$RTK_ASSET"
        RTK_TMP="$(mktemp -d 2>/dev/null || echo "$HOME/.cache/tmp/rtk_dl")"; mkdir -p "$RTK_TMP"
        info "  downloading rtk release ($RTK_ASSET)..."
        if { command -v curl >/dev/null 2>&1 && curl -fsSL "$RTK_URL" -o "$RTK_TMP/$RTK_ASSET"; } || { command -v wget >/dev/null 2>&1 && wget -q "$RTK_URL" -O "$RTK_TMP/$RTK_ASSET"; }; then
          tar -xzf "$RTK_TMP/$RTK_ASSET" -C "$RTK_TMP" 2>/dev/null
          RTK_SRC="$(find "$RTK_TMP" -name rtk -type f 2>/dev/null | head -1 || true)"
          if [ -n "$RTK_SRC" ] && [ -f "$RTK_SRC" ]; then
            mkdir -p "$(dirname "$RTK_BIN")"
            cp -f "$RTK_SRC" "$RTK_BIN" 2>/dev/null && chmod +x "$RTK_BIN" 2>/dev/null && RTK_CMD="$RTK_BIN" && ok "rtk installed to $RTK_BIN (from GitHub release)" || warn "rtk copy failed"
          else warn "rtk release archive did not contain the rtk binary"
          fi
        else warn "rtk download failed - install manually from https://github.com/rtk-ai/rtk/releases"
        fi
        rm -rf "$RTK_TMP" 2>/dev/null
      fi
    fi
    if [ -n "$RTK_CMD" ]; then RTK_STATUS="installed"; elif [ -z "$RTK_STATUS" ]; then RTK_STATUS="failed"; fi
  elif [ -z "$RTK_STATUS" ]; then
    RTK_STATUS="declined"
  fi
fi

# 3b. RTK agent wiring - RTK ships its own per-agent integrations (global hooks
#     for claude/cursor/gemini/codex/copilot, plugin for opencode; cline is
#     project-scoped .clinerules only). Hand off to RTK's own `rtk init` with the
#     RTK's own `rtk init -g`. Fail-open: never blocks the knocode install.
if [ -n "$AGENT_SEL" ] && [ -n "$RTK_CMD" ]; then
  info "Wiring RTK integrations for selected agents (external tool)..."
  if ! command -v rg >/dev/null 2>&1; then
    warn "ripgrep (rg) not on PATH - some rtk filters need it (apt/dnf/brew install ripgrep)"
  fi
  n=0
  total=$(echo $AGENT_SEL | wc -w | tr -d ' ')
  # Per-agent RTK flags (rtk-ai/rtk): claude is the default global hook,
  # cursor/gemini/codex/copilot/opencode take their own global flags. Cline has
  # NO global integration (prompt-level `.clinerules`, project-scoped) — skipped
  # with guidance. --auto-patch keeps every variant non-interactive.
  rtk_args_for() { # $1=agent -> prints rtk args, exits 1 when N/A
    case "$1" in
      opencode) echo "init -g --opencode --auto-patch" ;;
      copilot)  echo "init -g --copilot --auto-patch" ;;
      claude)   echo "init -g --auto-patch" ;;
      cursor)   echo "init -g --agent cursor --auto-patch" ;;
      gemini)   echo "init -g --gemini --auto-patch" ;;
      codex)    echo "init -g --codex --auto-patch" ;;
      *) return 1 ;;
    esac
  }
  if echo "$AGENT_SEL" | grep -qw cline; then
    skip "cline has no global RTK integration - run 'rtk init --agent cline' inside each project you open with Cline (writes .clinerules)"
  fi
  for a in $AGENT_SEL; do
    if ! _rtk_args="$(rtk_args_for "$a")"; then continue; fi
    n=$((n + 1))
    info "  [$n/$total] wiring rtk for $a (runs: rtk $_rtk_args - usually takes a few seconds)..."
    # stdin closed + output shown: rtk never waits silently on the installer's stdin,
    # and the user sees progress instead of a frozen prompt if it needs time.
    # shellcheck disable=SC2086
    rtk_out=$("$RTK_CMD" $_rtk_args </dev/null 2>&1)
    if [ $? -eq 0 ]; then
      ok "rtk integration wired for $a (rtk $_rtk_args)"
      # Relay rtk output minus its "/!\ No hook installed" upsell: the global hook
      # is installed right after this loop; the filter stays in case rtk still
      # prints the warning (e.g. the hook install failed). (awk, not grep -v | head:
      # pipefail-safe when every line matches.)
      echo "$rtk_out" | awk 'NF && $0 !~ /No hook installed/ { print; if (++n == 3) exit }' | sed 's/^/    /'
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
      warn "rtk init failed for $a (exit $?) - run manually: rtk $_rtk_args"
      echo "$rtk_out" | head -5 | sed 's/^/    /'
    fi
  done

  # ── Global hook: register RTK's compression hook (Claude-style hook + RTK.md)
  # so token savings also apply outside the wired agents. Fail-open; stdin closed
  # so rtk never waits on the installer's stdin. Note: init regenerates rtk.ts,
  # so the plugin probe is re-patched right after (idempotent).
  info "Installing RTK global hook (rtk init -g --auto-patch)..."
  rtk_hook_rc=0
  rtk_hook_out=$("$RTK_CMD" init -g --auto-patch </dev/null 2>&1) || rtk_hook_rc=$?
  if [ "$rtk_hook_rc" -eq 0 ]; then
    ok "rtk global hook installed (rtk init -g --auto-patch)"
    echo "$rtk_hook_out" | awk 'NF { print; if (++n == 3) exit }' | sed 's/^/    /'
  else
    warn "rtk global hook install failed - run manually: rtk init -g --auto-patch"
    echo "$rtk_hook_out" | head -5 | sed 's/^/    /'
  fi
  # Hook init may regenerate rtk.ts — re-apply the Windows-safe probe patch.
  RTK_OC_PLUGIN="$HOME/.config/opencode/plugins/rtk.ts"
  if [ -f "$RTK_OC_PLUGIN" ] && grep -q '`which rtk`' "$RTK_OC_PLUGIN"; then
    sed -i.bak 's/`which rtk`/`rtk --version`/' "$RTK_OC_PLUGIN" && rm -f "$RTK_OC_PLUGIN.bak"
    info "  [PATCH] opencode plugin probe re-applied after hook init (Windows-safe)"
  fi
  info "RTK wiring done."
fi

# 4. Start daemon - knocode must be in RUNNING state after installation
# TASK-037: launch from ~/.knocode/bin (installed copy), repo-independent working dir.
daemon_health() { curl -s -o /dev/null -m 2 http://127.0.0.1:9527/health; }
DAEMON_UP=no
if command -v curl >/dev/null 2>&1 && daemon_health; then
  DAEMON_UP=yes
  ok "knocode daemon already running at http://127.0.0.1:9527 (status: running)"
elif [ ! -x "$INSTALLED_DAEMON" ]; then
  warn "knocode-daemon binary not found at $INSTALLED_DAEMON - build first (cargo build --release) then re-run installer or start manually"
else
  # Stale processes (holding old binary/port but not answering /health) - stop them before restart
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
  if [ "$DAEMON_UP" = yes ]; then ok "knocode daemon RUNNING (http://127.0.0.1:9527, from $INSTALLED_DAEMON)"; else warn "daemon not responding on :9527 within 20s - start manually: $INSTALLED_DAEMON"; fi
fi

info "Done - daemon: $(if [ "$DAEMON_UP" = yes ]; then echo 'RUNNING at http://127.0.0.1:9527'; else echo "NOT running (start: $INSTALLED_DAEMON)"; fi) | agents: $(if [ -n "$AGENT_SEL" ]; then echo "$AGENT_SEL"; else echo none; fi) | rtk: ${RTK_STATUS:-unknown} | log verbosity: $VERBOSITY ($KNOCODE_LOG_LEVEL; logs: ~/.knocode/logs/knocode.log) | knocode doctor"
info "Docs: docs/*.md | knocode doctor"
