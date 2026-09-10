#!/usr/bin/env bash
# Knocode uninstaller (Unix: Linux/macOS, bash)
# V1 scope: local runtime only — reverses scripts/install.sh — removes everything by default (strict).
# Idempotent. Usage: bash scripts/uninstall.sh [--keep-external] [--keep-data] [--keep-build] [--force] [--dry-run]
# Default: remove binaries, plugins, ALL external tools and ALL data (prompts unless --force).
# Use --keep-* to preserve. Legacy --remove-external/--remove-data still accepted (now default).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEEP_EXTERNAL=false; KEEP_DATA=false; KEEP_BUILD=false; FORCE=false; DRY_RUN=false
REMOVE_EXTERNAL=false; REMOVE_DATA=false
for arg in "$@"; do case "$arg" in
  --keep-external) KEEP_EXTERNAL=true ;;
  --keep-data) KEEP_DATA=true ;;
  --keep-build) KEEP_BUILD=true ;;
  --remove-external) REMOVE_EXTERNAL=true ;;
  --remove-data) REMOVE_DATA=true ;;
  --force) FORCE=true ;;
  --dry-run|--whatif) DRY_RUN=true ;;
  -h|--help) echo "Usage: $0 [--keep-external] [--keep-data] [--keep-build] [--force] [--dry-run]"; echo "  Default: remove everything (strict). Use --keep-* to preserve."; exit 0 ;;
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
info "Removing opencode plugins..."
for pp in ".opencode/plugins/knocode.ts" "$HOME/.config/opencode/plugins/knocode.ts"; do
  check_pp="$pp"
  [ "$pp" = ".opencode/plugins/knocode.ts" ] && check_pp="$ROOT/.opencode/plugins/knocode.ts"
  if [ -e "$check_pp" ]; then if $DRY_RUN; then skip "would rm plugin 'knocode'"; else rm -f "$check_pp" && ok "removed plugin 'knocode'"; fi; else skip "not found plugin 'knocode'"; fi
done
# 3b. Opencode npm plugin — installed into GLOBAL ~/.config/opencode/node_modules (and legacy .opencode/node_modules)
info "Removing opencode npm plugin (opencode-knocode)..."
for pp in "$HOME/.config/opencode/node_modules/opencode-knocode" "$HOME/.config/opencode/node_modules/@opencode-ai" "$HOME/.config/opencode/package-lock.json" "$HOME/.cache/opencode/node_modules/opencode-knocode" ".opencode/node_modules/opencode-knocode" ".opencode/node_modules/@opencode-ai" ".opencode/package-lock.json"; do
  case "$pp" in /*) check_pp="$pp" ;; *) check_pp="$ROOT/$pp" ;; esac
  if [ -e "$check_pp" ]; then if $DRY_RUN; then skip "would rm -rf $pp"; else rm -rf "$check_pp" && ok "removed $pp (opencode npm plugin)"; fi; else skip "not found $pp"; fi
done
# Clean package.json deps: GLOBAL ~/.config/opencode/package.json + legacy .opencode/package.json — remove opencode-knocode dep or delete if only that
for pj in "$HOME/.config/opencode/package.json" "$ROOT/.opencode/package.json"; do
  if [ -f "$pj" ]; then
    if $DRY_RUN; then skip "would clean $pj (remove opencode-knocode dep)"; else
      if command -v node >/dev/null 2>&1; then
        PKG_JSON_PATH="$pj" node -e "const fs=require('fs');const p=process.env.PKG_JSON_PATH;try{let j=JSON.parse(fs.readFileSync(p,'utf8'));let changed=false;if(j.dependencies&&j.dependencies['opencode-knocode']){delete j.dependencies['opencode-knocode'];changed=true}if(j.dependencies&&Object.keys(j.dependencies).length===0){fs.unlinkSync(p);console.log('removed empty '+p)}else if(changed){fs.writeFileSync(p,JSON.stringify(j,null,2));console.log('cleaned opencode-knocode from '+p)} }catch(e){}" 2>/dev/null && ok "cleaned $pj" || warn "failed to clean $pj"
      else
        # fallback: remove file if it only contains opencode-knocode
        if grep -q "opencode-knocode" "$pj" 2>/dev/null && [ "$(wc -l < "$pj")" -lt 10 ]; then rm -f "$pj" && ok "removed $pj"; fi
      fi
    fi
  else skip "not found $pj"; fi
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
    j.plugin=j.plugin.filter(x=>x!=='opencode-knocode' && x!=='knocode' && !(typeof x==='string' && x.includes('opencode-knocode')));
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
  # rtk: unified ~/.knocode/bin + legacy ~/bin + cargo install
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
