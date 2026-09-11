#!/usr/bin/env bash
# Knocode compile script (Unix: Linux/macOS, bash)
# Builds knocode binaries from source. Installer assumes pre-compiled and does NOT build.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
RELEASE=true; SKIP_TESTS=false; FEATURES=""
for arg in "$@"; do case "$arg" in
  --debug) RELEASE=false ;;
  --skip-tests) SKIP_TESTS=true ;;
  --features) FEATURES="$2"; shift ;;
  --features=*) FEATURES="${arg#--features=}" ;;
  -h|--help) echo "Usage: $0 [--debug] [--skip-tests] [--features FEATS]"; echo "  --debug        Build debug (default: release)"; echo "  --skip-tests   Skip cargo test"; echo "  --features     Cargo features (e.g. extended-languages)"; exit 0 ;;
esac; done
info(){ echo -e "\033[36m[knocode]\033[0m $*"; } ; ok(){ echo -e "  \033[32m[OK]\033[0m $*"; } ; warn(){ echo -e "  \033[33m[WARN]\033[0m $*"; }
command -v cargo >/dev/null || { echo "cargo not found"; exit 1; }
command -v rustc >/dev/null || { echo "rustc not found"; exit 1; }
info "Compiling knocode ($([ "$RELEASE" = true ] && echo release || echo debug)) from $ROOT ..."
if [ -n "$FEATURES" ]; then info "Features: $FEATURES"; fi
if [ "$RELEASE" = true ]; then cargo build --release ${FEATURES:+--features "$FEATURES"}; ok "target/release/knocode + knocode-daemon"; else cargo build ${FEATURES:+--features "$FEATURES"}; ok "target/debug/knocode + knocode-daemon"; fi
if [ "$SKIP_TESTS" = true ]; then info "Skipping tests (--skip-tests)"; else info "cargo test --workspace --quiet ..."; cargo test --workspace --quiet ${FEATURES:+--features "$FEATURES"} && ok "tests passing" || warn "cargo test had failures"; fi
# --- opencode-knocode npm plugin ---
if command -v npm >/dev/null 2>&1; then
  if [ -d "$ROOT/packages/opencode-knocode" ]; then
    info "Building npm plugin packages/opencode-knocode ..."
    (
      cd "$ROOT/packages/opencode-knocode"
      if [ -f package-lock.json ]; then npm ci --silent || npm install --silent; else npm install --silent; fi
      npm run build --silent && ok "opencode-knocode dist built" || { warn "opencode-knocode build failed"; exit 0; }
      if [ "$SKIP_TESTS" = true ]; then info "Skipping opencode-knocode tests (--skip-tests)"; else npm test --silent && ok "opencode-knocode tests passing" || warn "opencode-knocode tests had failures"; fi
    )
  else
    warn "packages/opencode-knocode not found - skipping npm build"
  fi
else
  warn "npm not found - skipping opencode-knocode build (install Node.js 18+)"
fi
info "Compile done. Next: bash scripts/install.sh  (uses pre-compiled binary, no rebuild)"
