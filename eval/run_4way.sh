#!/bin/bash
# 2-Way Comparison: MCP-only vs Full Local Pipeline
# Usage: ./eval/run_4way.sh [eshop_repo_path]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${1:-C:/LeonRepository/eShopOnWeb}"
BINARY="C:/LeonRepository/knocode/target/release/knocode.exe"
RESULTS_DIR="$SCRIPT_DIR/results"
DATASET="$SCRIPT_DIR/datasets/eshop_tasks.yaml"

if [ ! -f "$BINARY" ]; then echo "ERROR: Binary not found"; exit 1; fi
mkdir -p "$RESULTS_DIR"

reindex() {
    rm -rf "$HOME/.knocode/index" 2>/dev/null
    cd "$REPO" && KNOCODE_SYMBOLS_ENABLED=$1 C:/LeonRepository/knocode/target/release/knocode.exe init 2>&1 | grep -E "Tantivy|Symbols|Graph" | head -5
}

run_eval() {
    KNOCODE_MCP_ENABLED=$2 KNOCODE_SYMBOLS_ENABLED=$3 \
        python3 "$SCRIPT_DIR/metrics/retrieval.py" \
        --dataset "$DATASET" --repo "$REPO" --binary "$BINARY" \
        --out "$RESULTS_DIR/$4" --timeout 30 2>&1 | grep "Summary:" -A8
}

extract() {
    python3 -c "
import json
with open('$RESULTS_DIR/$1') as f: d = json.load(f)
s = d.get('summary', {})
r5 = s.get('avg_recall@5', 0); r10 = s.get('avg_recall@10', 0)
mrr = s.get('avg_mrr', 0); lat = s.get('avg_latency_ms', 0)
ma = s.get('miss_analysis', {}); misses = sum(ma.values()) if ma else 0
print(f'$2| {r5:.4f} | {r10:.4f} | {mrr:.4f} | {lat:.0f}ms | {misses}')
"
}

echo "╔══════════════════════════════════════════════════════════════════════╗"
echo "║       2-WAY COMPARISON: MCP-only vs Local Pipeline (48 tasks)      ║"
echo "╚══════════════════════════════════════════════════════════════════════╝"
echo ""

# Config 1: MCP-only (graph search, no local BM25 symbols)
echo "━━━ [1/2] codebase-memory-mcp only ━━━"
reindex true
run_eval "codebase-memory-mcp" "true" "true" "mcp_only.json"
echo ""

# Config 2: Full Local Pipeline (BM25 + symbols + knowledge + skills)
echo "━━━ [2/2] Full Local Pipeline ━━━"
reindex true
run_eval "Full Pipeline" "false" "true" "full_local.json"
echo ""

echo "╔══════════════════════════════════════════════════════════════════════╗"
echo "║                         RESULTS                                     ║"
echo "╠══════════════════════════════════════════════════════════════════════╣"
printf "║ %-30s │ %-8s │ %-8s │ %-7s │ %-8s │ %-7s ║\n" "Config" "R@5" "R@10" "MRR" "Latency" "Misses"
echo "╠══════════════════════════════════════════════════════════════════════╣"
extract "mcp_only.json"  "codebase-memory-mcp"
extract "full_local.json" "Full Pipeline"
echo "╚══════════════════════════════════════════════════════════════════════╝"
echo ""
echo "Results: $RESULTS_DIR/"
