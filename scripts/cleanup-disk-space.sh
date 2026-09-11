#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# cleanup-disk-space.sh — Free disk space for this project (safe, dry-run first)
#
# Usage:
#   bash scripts/cleanup-disk-space.sh          # dry-run (shows what would be deleted)
#   bash scripts/cleanup-disk-space.sh --apply  # actually delete
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

DRY_RUN=true
if [[ "${1:-}" == "--apply" ]]; then
    DRY_RUN=false
fi

# ── Helpers ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

freed=0

dir_size() {
    # Fast approximate size: count files and estimate, or use timeout'd du
    local path="$1"
    if [[ ! -e "$path" ]]; then echo 0; return; fi
    # Try du with a 5s timeout per directory
    timeout 5 du -sm "$path" 2>/dev/null | awk '{print $1}' || {
        # Fallback: count files and estimate ~50KB avg
        local count
        count=$(find "$path" -type f 2>/dev/null | head -5000 | wc -l)
        echo $(( count / 20 ))  # rough estimate in MB
    }
}

remove_if_exists() {
    local path="$1"
    local label="$2"
    if [[ -e "$path" ]]; then
        local size
        size=$(dir_size "$path")
        freed=$((freed + size))
        if $DRY_RUN; then
            echo -e "  ${YELLOW}[DRY-RUN]${NC} would remove ${CYAN}${path}${NC} (~${size} MB)"
        else
            echo -e "  ${RED}Removing${NC} ${path} (~${size} MB)..."
            rm -rf "$path"
            echo -e "  ${GREEN}✓ Removed${NC}"
        fi
    fi
}

# ── Detect environment ───────────────────────────────────────────────────────
IS_WINDOWS=false
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "cygwin" || "$OSTYPE" == "win32" || -n "${USERPROFILE:-}" ]]; then
    IS_WINDOWS=true
fi

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${CYAN}  Disk Space Cleanup for: ${PROJECT_ROOT}${NC}"
echo -e "${CYAN}  Mode: $(if $DRY_RUN; then echo 'DRY-RUN (preview only)'; else echo 'APPLY (deleting files!)'; fi)${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# 1. PROJECT BUILD ARTIFACTS
# ═══════════════════════════════════════════════════════════════════════════════
echo -e "${GREEN}[1/5] Project build artifacts${NC}"
remove_if_exists "target/"           "Cargo target/ (build artifacts)"
remove_if_exists "experiments/ast-grep-tree-sitter-interop/target/" "Experiments build artifacts"
remove_if_exists "node_modules/"     "Node.js dependencies"
echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# 2. CARGO HOME CACHE (registry + git checkouts)
# ═══════════════════════════════════════════════════════════════════════════════
echo -e "${GREEN}[2/5] Cargo home cache${NC}"
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
remove_if_exists "$CARGO_HOME/registry/cache/"   "Cargo registry cache (downloaded .crate files)"
remove_if_exists "$CARGO_HOME/registry/src/"     "Cargo registry source (extracted crates)"
remove_if_exists "$CARGO_HOME/git/checkouts/"    "Cargo git dependency checkouts"
remove_if_exists "$CARGO_HOME/git/db/"           "Cargo git dependency databases"

# Smarter option: just run cargo clean which is safer
echo -e "  ${CYAN}Alternative: cargo clean (removes only project target/, keeps cargo cache)${NC}"
echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# 3. RUSTUP TOOLCHAINS (old/unused versions)
# ═══════════════════════════════════════════════════════════════════════════════
echo -e "${GREEN}[3/5] Rust toolchain cleanup hints${NC}"
if command -v rustup &>/dev/null; then
    echo -e "  ${CYAN}Installed toolchains:${NC}"
    rustup toolchain list
    echo ""
    echo -e "  ${YELLOW}Tip:${NC} To remove old toolchains: ${CYAN}rustup toolchain uninstall <name>${NC}"
    echo -e "  ${YELLOW}Tip:${NC} To remove unused components: ${CYAN}rustup component remove <name>${NC}"
    echo -e "  ${YELLOW}Tip:${NC} To clean downloaded archives: ${CYAN}rm -rf $CARGO_HOME/archive/${NC}"
    remove_if_exists "$CARGO_HOME/archive/"  "Cargo downloaded toolchain archives"
else
    echo -e "  ${CYAN}rustup not found, skipping${NC}"
fi
echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# 4. WINDOWS TEMP + RECYCLE BIN
# ═══════════════════════════════════════════════════════════════════════════════
echo -e "${GREEN}[4/5] Windows temp + recycle bin${NC}"
if $IS_WINDOWS; then
    # User temp folder
    TEMP_DIR="${LOCALAPPDATA:-$USERPROFILE/AppData/Local}/Temp"
    if [[ -d "$TEMP_DIR" ]]; then
        echo -e "  ${CYAN}Windows temp folder:${NC} $TEMP_DIR"
        if $DRY_RUN; then
            temp_size=$(dir_size "$TEMP_DIR")
            echo -e "  ${YELLOW}[DRY-RUN]${NC} Would clean temp files (~${temp_size} MB)"
            echo -e "  ${YELLOW}[DRY-RUN]${NC} Use: ${CYAN}PowerShell: Remove-Item '$TEMP_DIR\\*' -Recurse -Force${NC}"
        else
            echo -e "  ${YELLOW}Cleaning user temp (files older than 7 days)...${NC}"
            find "$TEMP_DIR" -mindepth 1 -maxdepth 1 -mtime +7 -exec rm -rf {} + 2>/dev/null || true
            echo -e "  ${GREEN}✓ Temp cleaned${NC}"
        fi
    fi

    # Recycle bin
    echo ""
    echo -e "  ${CYAN}Recycle Bin:${NC}"
    if $DRY_RUN; then
        echo -e "  ${YELLOW}[DRY-RUN]${NC} Would empty recycle bin"
        echo -e "  ${YELLOW}[DRY-RUN]${NC} Use: ${CYAN}PowerShell: Clear-RecycleBin -Force${NC}"
    else
        echo -e "  ${YELLOW}Emptying recycle bin...${NC}"
        PowerShell.exe -NoProfile -Command "Clear-RecycleBin -Force" 2>/dev/null || true
        echo -e "  ${GREEN}✓ Recycle bin emptied${NC}"
    fi

    # npm/yarn cache
    echo ""
    echo -e "  ${CYAN}Package manager caches:${NC}"
    NPM_CACHE="$(npm config get cache 2>/dev/null || echo "")"
    if [[ -n "$NPM_CACHE" && -d "$NPM_CACHE" ]]; then
        remove_if_exists "$NPM_CACHE" "npm cache"
    fi
else
    echo -e "  ${CYAN}Not Windows, skipping${NC}"
    # Linux/macOS temp
    remove_if_exists "/tmp/knocode-*" "Knocode temp files"
fi
echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# 5. MISCELLANEOUS
# ═══════════════════════════════════════════════════════════════════════════════
echo -e "${GREEN}[5/5] Miscellaneous${NC}"
remove_if_exists ".opencode/cache/"    "OpenCode cache"
remove_if_exists ".knocode/cache/"     "Knocode cache"
remove_if_exists "eval/results/"       "Evaluation results"
remove_if_exists "experiments/"        "All experiments (already in .gitignore)"
echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# SUMMARY
# ═══════════════════════════════════════════════════════════════════════════════
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"
if $DRY_RUN; then
    echo -e "  ${YELLOW}Estimated space to free: ~${freed} MB${NC}"
    echo ""
    echo -e "  To actually delete, run:"
    echo -e "    ${CYAN}bash scripts/cleanup-disk-space.sh --apply${NC}"
else
    echo -e "  ${GREEN}Space freed: ~${freed} MB${NC}"
fi
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"
echo ""
