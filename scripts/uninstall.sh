#!/usr/bin/env bash
# Knocode uninstaller (Unix: Linux/macOS, bash)
# V1 scope: local runtime only — reverses scripts/install.sh — removes everything by default (strict).
# Also runs standalone (download uninstall.sh from the latest GitHub Release) —
# repository steps are skipped when no source checkout is present.
# Idempotent. Usage: bash scripts/uninstall.sh [--keep-external] [--keep-data] [--keep-build] [--remove-repo] [--force] [--dry-run]
# Default: remove binaries, plugins, ALL external tools and ALL data (prompts unless --force).
# Repository files (.opencode/plugins/, target/, .knocode/) are NEVER deleted unless --remove-repo.
# Use --keep-* to preserve. Legacy --remove-external/--remove-data still accepted (now default).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEEP_EXTERNAL=false; KEEP_DATA=false; KEEP_BUILD=false; FORCE=false; DRY_RUN=false
REMOVE_EXTERNAL=false; REMOVE_DATA=false; REMOVE_REPO=false
for arg in "$@"; do case "$arg" in
  --keep-external) KEEP_EXTERNAL=true ;;
  --keep-data) KEEP_DATA=true ;;
  --keep-build) KEEP_BUILD=true ;;
  --remove-external) REMOVE_EXTERNAL=true ;;
  --remove-data) REMOVE_DATA=true ;;
  --remove-repo) REMOVE_REPO=true ;;
  --force) FORCE=true ;;
  --dry-run|--whatif) DRY_RUN=true ;;
  -h|--help) echo "Usage: $0 [--keep-external] [--keep-data] [--keep-build] [--remove-repo] [--force] [--dry-run]"; echo "  Default: remove everything except repository files. Use --keep-* to preserve, --remove-repo to also delete repository artifacts."; exit 0 ;;
esac; done
# Default is to remove everything unless --keep-* is set (legacy --remove-* also triggers)
if [ "$KEEP_EXTERNAL" = false ]; then REMOVE_EXTERNAL=true; fi
if [ "$KEEP_DATA" = false ]; then REMOVE_DATA=true; fi
if [ "$REMOVE_EXTERNAL" = true ]; then KEEP_EXTERNAL=false; fi
if [ "$REMOVE_DATA" = true ]; then KEEP_DATA=false; fi
# Re-derive effective flags
DO_REMOVE_EXTERNAL=true; [ "$KEEP_EXTERNAL" = true ] && DO_REMOVE_EXTERNAL=false
DO_REMOVE_DATA=true; [ "$KEEP_DATA" = true ] && DO_REMOVE_DATA=false

info(){ echo -e "\033[36m[knocode]\033[0m $*"; }
ok(){ echo -e "  \033[32m[OK]\033[0m $*"; }
warn(){ echo -e "  \033[33m[WARN]\033[0m $*"; }
skip(){ echo -e "  \033[90m[SKIP]\033[0m $*"; }
run(){ if $DRY_RUN; then skip "would $*"; else eval "$*"; fi; }

info "Knocode uninstaller"
$DRY_RUN && warn "DryRun active - no changes will be made"
info "Options: RemoveExternal(effective=$DO_REMOVE_EXTERNAL KeepExternal=$KEEP_EXTERNAL) RemoveData(effective=$DO_REMOVE_DATA KeepData=$KEEP_DATA) KeepBuild=$KEEP_BUILD Force=$FORCE"

if $DO_REMOVE_DATA && ! $FORCE && ! $DRY_RUN; then
  read -p "This will permanently delete ~/.knocode and .knocode/ (project config). Continue? [y/N] " ans
  case "$ans" in y|Y|yes|YES) ;; *) info "Aborted."; exit 0;; esac
fi

# 1. Stop daemon / socket
info "Stopping daemon and cleaning socket..."
for p in knocode-daemon knocode; do
  if pgrep -x "$p" >/dev/null 2>&1; then
    if $DRY_RUN; then skip "would pkill $p"; else pkill -TERM "$p" 2>/dev/null || true; ok "stopped $p"; fi
  else skip "no running $p process"; fi
done
for sock in "$HOME/.knocode/knocode.sock" "/tmp/knocode.sock" ".knocode/knocode.sock" "/tmp/knocode.sock.lock"; do
  # Use absolute for check but log relative for repo file
  check_sock="$sock"
  [ "$sock" = ".knocode/knocode.sock" ] && check_sock="$ROOT/.knocode/knocode.sock"
  if [ -e "$check_sock" ]; then
    if $DRY_RUN; then skip "would rm $sock"; else rm -f "$check_sock" && ok "removed socket $sock" || warn "failed $sock"; fi
  fi
done

# 1b. TASK-037: remove installed binaries (~/.knocode/bin) + revert PATH entries.
# Always executed: PATH is shell state, independent of --keep-data/--remove-repo.
info "Removing installed knocode binaries from ~/.knocode/bin..."
for bin in "$HOME/.knocode/bin/knocode" "$HOME/.knocode/bin/knocode-daemon"; do
  if [ -e "$bin" ]; then if $DRY_RUN; then skip "would rm $bin"; else rm -f "$bin" && ok "removed $bin"; fi; else skip "not found $bin"; fi
done
if [ -d "$HOME/.knocode/bin" ] && [ -z "$(ls -A "$HOME/.knocode/bin" 2>/dev/null)" ]; then
  if $DRY_RUN; then skip "would rmdir ~/.knocode/bin"; else rmdir "$HOME/.knocode/bin" 2>/dev/null && ok "removed empty ~/.knocode/bin/"; fi
fi
# Revert PATH: drop our marker block from ~/.profile and ~/.bashrc (idempotent)
for rc in "$HOME/.profile" "$HOME/.bashrc"; do
  if [ -f "$rc" ] && grep -qs "KNOCODE_BIN_PATH" "$rc"; then
    if $DRY_RUN; then skip "would remove knocode PATH lines from $rc"
    else
      sed -i '/# KNOCODE_BIN_PATH/d; /\.knocode\/bin/d' "$rc" 2>/dev/null && ok "removed knocode PATH entry from $rc" || warn "failed to edit $rc"
    fi
  else skip "no knocode PATH entry in $rc"; fi
done

# 2. Build artifacts (use .opencode/.knocode relative, not absolute repo)
if $KEEP_BUILD; then info "Skipping build artifact removal (--keep-build)";
else
  info "Removing build artifacts..."
  for bin in "target/release/knocode" "target/release/knocode-daemon" "target/debug/knocode" "target/debug/knocode-daemon"; do
    if [ -e "$ROOT/$bin" ]; then if $DRY_RUN; then skip "would rm $bin"; else rm -f "$ROOT/$bin" && ok "removed $bin"; fi; else skip "not found $bin"; fi
  done
  if $DO_REMOVE_DATA && [ -d "$ROOT/target" ]; then
    if $DRY_RUN; then skip "would rm -rf target/"; else rm -rf "$ROOT/target" && ok "removed target/ (cargo clean)"; fi
  else skip "keeping target/ cache (--keep-data)"; fi
fi

# 3. Plugins (use .opencode folder, never absolute repo) - plugin 'knocode'
# Global copy is an installed artifact - always delete. Repo copy only with --remove-repo.
info "Removing opencode plugins..."
if [ -e "$HOME/.config/opencode/plugins/knocode.ts" ]; then if $DRY_RUN; then skip "would rm plugin 'knocode'"; else rm -f "$HOME/.config/opencode/plugins/knocode.ts" && ok "removed plugin 'knocode'"; fi; else skip "not found plugin 'knocode'"; fi
if [ -e "$ROOT/.opencode/plugins/knocode.ts" ]; then
  if $REMOVE_REPO; then if $DRY_RUN; then skip "would rm plugin 'knocode' (--remove-repo)"; else rm -f "$ROOT/.opencode/plugins/knocode.ts" && ok "removed plugin 'knocode' (--remove-repo)"; fi
  else skip "keeping plugin 'knocode' (use --remove-repo to delete)"; fi
else skip "not found plugin 'knocode'"; fi
# RTK opencode file plugin (created by `rtk init -g --opencode`, no documented --uninstall)
if [ -e "$HOME/.config/opencode/plugins/rtk.ts" ]; then if $DRY_RUN; then skip "would rm plugin 'rtk' (rtk init artifact)"; else rm -f "$HOME/.config/opencode/plugins/rtk.ts" && ok "removed plugin 'rtk' (rtk init artifact)"; fi; else skip "not found plugin 'rtk'"; fi
if [ -e "$ROOT/.opencode/plugins/rtk.ts" ]; then
  if $REMOVE_REPO; then if $DRY_RUN; then skip "would rm plugin 'rtk' (--remove-repo)"; else rm -f "$ROOT/.opencode/plugins/rtk.ts" && ok "removed plugin 'rtk' (--remove-repo)"; fi
  else skip "keeping plugin 'rtk' (use --remove-repo to delete)"; fi
else skip "not found plugin 'rtk'"; fi
# 3b. Opencode plugin bundle — copied into GLOBAL ~/.config/opencode/node_modules
# by the release installer (self-contained dist, no npm). Legacy npm installs
# may also have left package.json deps behind - that is the user's own file
# now (knocode no longer creates it), so only the bundle dirs are removed.
info "Removing opencode plugin bundle (opencode-knocode)..."
for pp in "$HOME/.config/opencode/node_modules/opencode-knocode" "$HOME/.cache/opencode/node_modules/opencode-knocode" ".opencode/node_modules/opencode-knocode"; do
  case "$pp" in /*) check_pp="$pp" ;; *) check_pp="$ROOT/$pp" ;; esac
  if [ -e "$check_pp" ]; then if $DRY_RUN; then skip "would rm -rf $pp"; else rm -rf "$check_pp" && ok "removed $pp (opencode plugin bundle)"; fi; else skip "not found $pp"; fi
done
# Remove empty .opencode dir if only empty after plugin removal (keep if has other config)
if [ -d "$ROOT/.opencode" ] && [ -z "$(ls -A "$ROOT/.opencode" 2>/dev/null)" ]; then
  if $DRY_RUN; then skip "would rmdir .opencode (empty)"; else rmdir "$ROOT/.opencode" 2>/dev/null && ok "removed empty .opencode/"; fi
else skip "keeping .opencode/ (has opencode.jsonc or other config)"; fi
# 3c. Knocode agent skill (opencode) - global ~/.config/opencode/skills/knocode (installed artifact, always)
info "Removing opencode agent skill (knocode)..."
OC_SKILL_GLOBAL="$HOME/.config/opencode/skills/knocode"
if [ -e "$OC_SKILL_GLOBAL" ]; then
  if $DRY_RUN; then skip "would rm -rf $OC_SKILL_GLOBAL (knocode skill)"; else rm -rf "$OC_SKILL_GLOBAL" && ok "removed $OC_SKILL_GLOBAL (knocode skill)"; fi
else skip "not found $OC_SKILL_GLOBAL (knocode skill)"; fi
# Remove empty global skills dir if only knocode was there
if [ -d "$HOME/.config/opencode/skills" ] && [ -z "$(ls -A "$HOME/.config/opencode/skills" 2>/dev/null)" ]; then
  if $DRY_RUN; then skip "would rmdir $HOME/.config/opencode/skills (empty)"; else rmdir "$HOME/.config/opencode/skills" 2>/dev/null && ok "removed empty global skills dir"; fi
fi
# 3d. Knocode Copilot artifacts (created by all installers when copilot is wired).
# Hooks file + skill are installed artifacts - always delete. The Agent Plugin dir
# (~/.knocode/copilot-plugin) lives under global data and is removed with it.
info "Removing Copilot hooks and skill (knocode)..."
if [ -e "$HOME/.copilot/hooks/knocode-context.json" ]; then
  if $DRY_RUN; then skip "would rm $HOME/.copilot/hooks/knocode-context.json (knocode hooks)"; else rm -f "$HOME/.copilot/hooks/knocode-context.json" && ok "removed $HOME/.copilot/hooks/knocode-context.json (knocode hooks)"; fi
else skip "not found $HOME/.copilot/hooks/knocode-context.json (knocode hooks)"; fi
if [ -d "$HOME/.copilot/hooks" ] && [ -z "$(ls -A "$HOME/.copilot/hooks" 2>/dev/null)" ]; then
  if $DRY_RUN; then skip "would rmdir $HOME/.copilot/hooks (empty)"; else rmdir "$HOME/.copilot/hooks" 2>/dev/null && ok "removed empty Copilot hooks dir"; fi
fi
if [ -e "$HOME/.copilot/skills/knocode" ]; then
  if $DRY_RUN; then skip "would rm -rf $HOME/.copilot/skills/knocode (knocode skill)"; else rm -rf "$HOME/.copilot/skills/knocode" && ok "removed $HOME/.copilot/skills/knocode (knocode skill)"; fi
else skip "not found $HOME/.copilot/skills/knocode (knocode skill)"; fi
if [ -d "$HOME/.copilot/skills" ] && [ -z "$(ls -A "$HOME/.copilot/skills" 2>/dev/null)" ]; then
  if $DRY_RUN; then skip "would rmdir $HOME/.copilot/skills (empty)"; else rmdir "$HOME/.copilot/skills" 2>/dev/null && ok "removed empty Copilot skills dir"; fi
fi
# 3g. Universal agents (claude/cursor/gemini/codex/cline): skill dirs + `knocode`
# MCP entries. Config files are NEVER deleted — only knocode keys are removed.
info "Removing universal agent wiring (knocode)..."
uni_skill_rm() { # $1=agent $2=skill dir
  if [ -e "$2" ]; then if $DRY_RUN; then skip "would rm -rf $2 ($1 skill)"; else rm -rf "$2" && ok "removed $1 skill ($2)"; fi; else skip "not found $1 skill ($2)"; fi
  _parent="$(dirname "$2")"
  if [ -d "$_parent" ] && [ -z "$(ls -A "$_parent" 2>/dev/null)" ]; then
    if $DRY_RUN; then skip "would rmdir $_parent (empty)"; else rmdir "$_parent" 2>/dev/null && ok "removed empty $1 skills dir"; fi
  fi
}
uni_mcp_json_rm() { # $1=agent $2=config path (drop mcpServers.knocode, keep file + rest)
  if [ ! -f "$2" ]; then skip "MCP config not found at $2"; return; fi
  if ! command -v node >/dev/null 2>&1; then skip "node not available - cannot clean $2"; return; fi
  if $DRY_RUN; then skip "would clean knocode MCP entry from $2"; return; fi
  MCP_CFG="$2" node -e "
const fs=require('fs');const p=process.env.MCP_CFG;
try{
  const raw=fs.readFileSync(p,'utf8');
  if(!raw.trim()){console.error('empty');process.exit(1);}
  let j;try{j=JSON.parse(raw);}catch{j=JSON.parse(raw.replace(/\/\/.*$/gm,'').replace(/\/\*[\s\S]*?\*\//g,'').replace(/,\s*([\}\]])/g,'\$1'));}
  if(!j.mcpServers||typeof j.mcpServers!=='object'||!('knocode' in j.mcpServers)){console.error('absent');process.exit(1);}
  delete j.mcpServers.knocode;
  if(Object.keys(j.mcpServers).length===0)delete j.mcpServers;
  fs.writeFileSync(p,JSON.stringify(j,null,2));console.log('removed');
}catch(e){console.error(e.message);process.exit(1);}
" 2>/dev/null && ok "removed knocode MCP entry from $2" || skip "no knocode MCP entry at $2"
}
uni_skill_rm claude "$HOME/.claude/skills/knocode"
uni_mcp_json_rm claude "$HOME/.claude.json"
uni_skill_rm cursor "$HOME/.cursor/skills/knocode"
uni_mcp_json_rm cursor "$HOME/.cursor/mcp.json"
uni_skill_rm gemini "$HOME/.gemini/skills/knocode"
uni_mcp_json_rm gemini "$HOME/.gemini/settings.json"
uni_skill_rm cline "$HOME/.cline/skills/knocode"
uni_mcp_json_rm cline "$HOME/.cline/data/settings/cline_mcp_settings.json"
# Codex: skill under ~/.knocode/skills + TOML entries in ~/.codex/config.toml
uni_skill_rm codex "$HOME/.knocode/skills/knocode"
CODEX_CFG="$HOME/.codex/config.toml"
if [ ! -f "$CODEX_CFG" ]; then skip "MCP config not found at $CODEX_CFG";
elif ! command -v node >/dev/null 2>&1; then skip "node not available - cannot clean $CODEX_CFG";
elif $DRY_RUN; then skip "would clean knocode entries from $CODEX_CFG";
else
  MCP_CFG="$CODEX_CFG" node -e "
const fs=require('fs');const p=process.env.MCP_CFG;
try{
  const lines=fs.readFileSync(p,'utf8').split(/\r?\n/);
  const bounds=[];for(let i=0;i<lines.length;i++)if(/^\s*\[/.test(lines[i]))bounds.push(i);
  const out=bounds.length&&bounds[0]>0?lines.slice(0,bounds[0]):[];
  bounds.push(lines.length);let changed=false;
  for(let b=0;b<bounds.length-1;b++){
    const block=lines.slice(bounds[b],bounds[b+1]);const head=lines[bounds[b]];let drop=false;
    if(/^\s*\[mcp_servers\.knocode\]/.test(head))drop=true;
    else if(/^\s*\[\[skills\.config\]\]/.test(head))drop=/knocode/.test(block.join('\n'));
    if(drop)changed=true;else out.push(...block);
  }
  if(!changed){console.error('absent');process.exit(1);}
  fs.writeFileSync(p,out.join('\n'));console.log('removed');
}catch(e){console.error(e.message);process.exit(1);}
" 2>/dev/null && ok "removed knocode entries from $CODEX_CFG" || skip "no knocode entries at $CODEX_CFG"
fi
# Shared MCP bridge (installed artifact like binaries - always remove).
if [ -e "$HOME/.knocode/mcp-server" ]; then if $DRY_RUN; then skip "would rm -rf ~/.knocode/mcp-server (MCP bridge)"; else rm -rf "$HOME/.knocode/mcp-server" && ok "removed shared MCP bridge (~/.knocode/mcp-server)"; fi; else skip "not found shared MCP bridge (~/.knocode/mcp-server)"; fi
# 3e. VS Code extension + Agent Plugin registration + prompt cache.
# The @knocode VSIX is installed via `code --install-extension`; fall back to the
# extension dir when the CLI is missing so the participant can't survive uninstall.
# settings.json entries (chat.pluginLocations / enabledPlugins) survive the source-dir
# removal and keep VS Code loading stale hooks/skill — clean knocode keys only.
info "Removing VS Code extension and Agent Plugin registration (knocode)..."
if command -v code >/dev/null 2>&1; then
  if $DRY_RUN; then skip "would run code --uninstall-extension knocode.knocode-copilot-extension"
  else code --uninstall-extension knocode.knocode-copilot-extension >/dev/null 2>&1 && ok "uninstalled VS Code extension knocode.knocode-copilot-extension" || warn "VS Code extension uninstall failed (fail-open)"; fi
else skip "VS Code CLI (code) not on PATH"; fi
for _extdir in "$HOME/.vscode/extensions" "$HOME/.vscode-insiders/extensions"; do
  for _d in "$_extdir"/knocode.knocode-copilot-extension-*; do
    [ -e "$_d" ] || continue
    if $DRY_RUN; then skip "would rm -rf $_d"; else rm -rf "$_d" && ok "removed VS Code extension dir $_d"; fi
  done
done
for _cfg in "$HOME/.config/Code/User/settings.json" "$ROOT/.vscode/settings.json"; do
  if [ ! -f "$_cfg" ]; then skip "settings not found at $_cfg"; continue; fi
  if $DRY_RUN; then skip "would clean knocode plugin registration from $_cfg"; continue; fi
  if command -v node >/dev/null 2>&1; then
    _CFG_PATH="$_cfg" node -e "
const fs=require('fs');const p=process.env._CFG_PATH;
try{
  let raw=fs.readFileSync(p,'utf8');
  let stripped=raw.replace(/\/\/.*$/gm,'').replace(/\/\*[\s\S]*?\*\//g,'');
  stripped=stripped.replace(/,\s*([\}\]])/g,'\$1');
  let j=JSON.parse(stripped); let changed=false;
  for(const key of ['chat.pluginLocations','enabledPlugins']){
    if(j[key] && typeof j[key]==='object' && !Array.isArray(j[key])){
      for(const k of Object.keys(j[key])){
        if(/knocode|copilot-plugin/i.test(k)){ delete j[key][k]; changed=true; }
      }
      if(Object.keys(j[key]).length===0) delete j[key];
    }
  }
  if(changed){ fs.writeFileSync(p,JSON.stringify(j,null,2)); console.log('cleaned'); }
} catch(e){ console.error(e.message); process.exit(1); }
" 2>/dev/null && ok "removed knocode plugin registration from $_cfg" || skip "no knocode plugin entries at $_cfg"
  else skip "node not available - cannot clean $_cfg"; fi
done
# Prompt-cache sidecars (UserPromptSubmit -> PreToolUse handoff). Stale files would
# inject one outdated context block after a reinstall — always safe to delete.
_TMPBASE="${TMPDIR:-/tmp}"
for _cd in "$_TMPBASE/knocode-hooks" "${PLUGIN_DATA:+$PLUGIN_DATA/knocode-hooks}"; do
  [ -n "$_cd" ] || continue
  if [ -e "$_cd" ]; then if $DRY_RUN; then skip "would rm -rf $_cd (prompt cache)"; else rm -rf "$_cd" && ok "removed prompt cache $_cd"; fi; else skip "not found prompt cache $_cd"; fi
done
# Clean opencode.jsonc plugin + mcp entries (always, even without --RemoveRepo — so plugin not showing after uninstall)
for cfg in "$ROOT/.opencode/opencode.jsonc" "$ROOT/.opencode/opencode.json" "$HOME/.config/opencode/opencode.jsonc" "$HOME/.config/opencode/opencode.json"; do
  # repo configs always cleaned; global only if DO_REMOVE_DATA
  is_global=false; case "$cfg" in *".config/opencode"*) is_global=true;; esac
  if $is_global && ! $DO_REMOVE_DATA; then skip "keeping global $cfg (use --remove-data to clean)"; continue; fi
  if [ -f "$cfg" ]; then
    if $DRY_RUN; then skip "would clean knocode plugin/mcp from $cfg"; else
      if command -v node >/dev/null 2>&1; then
        node -e "
const fs=require('fs');const p='$cfg';
try{
  let raw=fs.readFileSync(p,'utf8');
  // strip comments for jsonc
  let stripped=raw.replace(/\/\/.*$/gm,'').replace(/\/\*[\s\S]*?\*\//g,'');
  stripped=stripped.replace(/,\s*([\}\]])/g,'\$1');
  let j=JSON.parse(stripped);
  let changed=false;
  if(Array.isArray(j.plugin)){
    const orig=j.plugin.length;
    j.plugin=j.plugin.filter(x=>x!=='opencode-knocode' && x!=='knocode' && x!=='rtk' && !(typeof x==='string' && x.includes('opencode-knocode')));
    if(j.plugin.length!==orig) changed=true;
    if(j.plugin.length===0) delete j.plugin;
  }
  if(j.mcp && typeof j.mcp==='object'){
    let m=j.mcp;
    let had=false;
    for(const k of Object.keys(m)){
      if(k.toLowerCase().includes('knocode')){ delete m[k]; had=true; }
    }
    if(had) changed=true;
    if(Object.keys(m).length===0) delete j.mcp;
  }
  // if file now only has \$schema or empty, remove it; else write back
  const keys=Object.keys(j).filter(k=>k!=='\$schema');
  if(keys.length===0){
    fs.unlinkSync(p); console.log('removed empty '+p);
  } else if(changed){
    // preserve \$schema if existed
    if(raw.includes('\"\$schema\"') && !('\$schema' in j)) j['\$schema']='https://opencode.ai/config.json';
    fs.writeFileSync(p, JSON.stringify(j,null,2));
    console.log('cleaned knocode plugin/mcp from '+p);
  }
}catch(e){ console.error(e.message); process.exit(1); }
" 2>/dev/null && ok "cleaned knocode plugin/mcp from $cfg" || warn "failed to clean $cfg"
      else
        # fallback sed: remove plugin line and mcp blocks
        if grep -q "opencode-knocode" "$cfg" 2>/dev/null; then
          sed -i '/opencode-knocode/d' "$cfg" 2>/dev/null && ok "cleaned opencode-knocode from $cfg (sed)" || true
        fi
      fi
    fi
  fi
done
# Also check global npm plugin (if ever published)
if command -v npm >/dev/null 2>&1; then
  if npm list -g opencode-knocode >/dev/null 2>&1; then if $DRY_RUN; then skip "would npm uninstall -g opencode-knocode"; else npm uninstall -g opencode-knocode 2>/dev/null && ok "uninstalled opencode-knocode (npm -g)" || warn "npm uninstall opencode-knocode failed"; fi; else skip "opencode-knocode not installed globally (npm -g)"; fi
fi
# 4. External tools (default: remove)
if ! $DO_REMOVE_EXTERNAL; then info "Skipping external tools (--keep-external)";
else
  info "Removing external tools (strict default)..."
  # rtk integrations: official uninstall FIRST (fail-open, idempotent), binary delete after.
  # Mirrors the installer's per-agent map (global hook + opencode/copilot/cursor/
  # gemini/codex). Cursor/gemini/codex --uninstall shapes are symmetric guesses —
  # rtk rejects unknown combos with an error, which stays a warn, never fatal.
  # `rtk init -g --uninstall` covers the global hook only; opencode is file-based
  # (~/.config/opencode/plugins/rtk.ts, removed in section 3) with no documented --uninstall.
  # Cline is project-scoped .clinerules (never written by the installer) — nothing to remove.
  _rtk_exe=""
  for _cand in "$HOME/.knocode/bin/rtk" "$HOME/bin/rtk"; do if [ -x "$_cand" ] || [ -f "$_cand" ]; then _rtk_exe="$_cand"; break; fi; done
  if [ -z "$_rtk_exe" ] && command -v rtk >/dev/null 2>&1; then _rtk_exe="rtk"; fi
  if [ -n "$_rtk_exe" ]; then
    for _args in "init -g --uninstall" "init -g --opencode --uninstall" "init --uninstall --global --copilot" "init --uninstall --copilot" "init -g --agent cursor --uninstall" "init -g --gemini --uninstall" "init -g --codex --uninstall"; do
      if $DRY_RUN; then skip "would run rtk $_args";
      else "$_rtk_exe" $_args </dev/null >/dev/null 2>&1 && ok "ran rtk $_args" || warn "rtk $_args failed (fail-open)"; fi
    done
  else skip "rtk binary not found - skipping official rtk uninstall (file fallback in section 3 still runs)"; fi
  # rtk binaries: unified ~/.knocode/bin + legacy ~/bin + cargo install
  for _pp in "$HOME/.knocode/bin/rtk" "$HOME/bin/rtk"; do if [ -f "$_pp" ]; then if $DRY_RUN; then skip "would rm $_pp"; else rm -f "$_pp" && ok "removed $_pp"; fi; fi; done
  if [ ! -f "$HOME/.knocode/bin/rtk" ] && [ ! -f "$HOME/bin/rtk" ]; then skip "~/bin/rtk and ~/.knocode/bin/rtk not found"; fi
  if command -v rtk >/dev/null 2>&1; then if $DRY_RUN; then skip "would cargo uninstall rtk (legacy)"; else cargo uninstall rtk 2>/dev/null && ok "uninstalled rtk (legacy cargo)" || warn "rtk cargo uninstall failed"; fi; else skip "legacy cargo rtk not installed"; fi
  if command -v rustup >/dev/null 2>&1; then
    if $DRY_RUN; then skip "would rustup component remove clippy"; else rustup component remove clippy 2>/dev/null && ok "removed rustup component clippy" || warn "clippy remove failed"; fi
    skip "keeping rustup toolchain (never uninstall rustup)"
  else skip "rustup not installed"; fi
fi

# 5. Data (default: remove) - use .knocode relative
if ! $DO_REMOVE_DATA; then
  info "Skipping data removal (--keep-data)"
  info "  Kept: ~/.knocode and .knocode/"
else
  info "Removing data (strict default)..."
  for d in "$HOME/.knocode" "$ROOT/.knocode"; do
    disp="$d"
    [ "$d" = "$ROOT/.knocode" ] && disp=".knocode/"
    if [ -d "$d" ] || [ -f "$d" ]; then if $DRY_RUN; then skip "would rm -rf $disp"; else rm -rf "$d" && ok "removed $disp"; fi; else skip "not found $disp"; fi
  done
fi

info "Uninstall complete."
! $DO_REMOVE_EXTERNAL && info "  External tools were kept (--keep-external)" || info "  External tools removed"
! $DO_REMOVE_DATA && info "  Data was kept (--keep-data)" || info "  Data removed"
$KEEP_BUILD && info "  Build artifacts were kept (--keep-build)" || info "  Build artifacts removed"
info "To reinstall: bash scripts/install.sh"
