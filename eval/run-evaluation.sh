#!/bin/bash
# Knocode Evaluation Runner
# Usage: ./run-evaluation.sh [model|context|all] [--view]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Parse arguments
SUITE="${1:-all}"
VIEW=false

if [[ "$2" == "--view" ]] || [[ "$1" == "--view" ]]; then
  VIEW=true
  SUITE="${2:-all}"
fi

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}=== Knocode AI Runtime Evaluation ===${NC}"
echo ""

TOTAL_PASS=0
TOTAL_FAIL=0

run_eval() {
  local name=$1
  local config=$2
  
  echo -e "${YELLOW}Running $name evaluation...${NC}"
  if npx promptfoo eval -c "$config" --no-cache 2>&1 | grep -E "Results:|passed|failed|errors"; then
    echo ""
  fi
}

case "$SUITE" in
  context|all)
    run_eval "Context Quality" "promptfooconfig.yaml"
    ;;
  *)
    echo "Usage: $0 [context|all] [--view]  (model routing removed — see docs/01-architecture/REMOVED_TOOLS.md)"
    exit 1
    ;;
esac

if [ "$VIEW" = true ]; then
  echo -e "${YELLOW}Opening results viewer...${NC}"
  npx promptfoo view
fi

echo -e "${GREEN}=== Evaluation Complete ===${NC}"
