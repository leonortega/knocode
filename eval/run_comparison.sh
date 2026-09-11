#!/bin/bash
# Retrieval Quality Comparison: BM25 vs codebase-memory-mcp
# Runs the 48-task eval dataset across two configurations and outputs a comparison table.
#
# Usage: ./run_comparison.sh [eshop_repo_path]
#
# Note: FlashRank was removed from the v1 runtime path per benchmark evaluation.
# See crates/knocode-knowledge/src/rerank.rs for rationale.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${1:-C:/LeonRepository/eShopOnWeb}"
BINARY="target/release/knocode.exe"
RESULTS_DIR="$SCRIPT_DIR/results"

# Ensure binary exists
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY — run 'cargo build --release -p knocode-cli' first"
    exit 1
fi

# Ensure eShopOnWeb is indexed
echo "=== Checking eShopOnWeb index ==="
if [ ! -d "$REPO/.knocode" ]; then
    echo "Indexing eShopOnWeb..."
    cd "$REPO" && "$BINARY" init 2>&1 | tail -5
fi

mkdir -p "$RESULTS_DIR"

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║       Retrieval Quality Comparison (48-task eShopOnWeb)     ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# ── Configuration 1: Baseline BM25 (no symbols, no MCP) ────────
echo "━━━ [1/3] Baseline BM25 (no symbols, no MCP) ━━━"
KNOCODE_MCP_ENABLED=false KNOCODE_SYMBOLS_ENABLED=false \
    python3 "$SCRIPT_DIR/metrics/retrieval.py" \
        --dataset "$SCRIPT_DIR/datasets/eshop_tasks.yaml" \
        --repo "$REPO" \
        --binary "$BINARY" \
        --out "$RESULTS_DIR/baseline_bm25.json" \
        --timeout 30 \
        --diag 2>&1 | tee "$RESULTS_DIR/baseline_bm25.log"
echo ""

# ── Configuration 2: BM25 + tree-sitter symbols ────────────────
echo "━━━ [2/3] BM25 + tree-sitter symbols ━━━"
KNOCODE_MCP_ENABLED=false KNOCODE_SYMBOLS_ENABLED=true \
    python3 "$SCRIPT_DIR/metrics/retrieval.py" \
        --dataset "$SCRIPT_DIR/datasets/eshop_tasks.yaml" \
        --repo "$REPO" \
        --binary "$BINARY" \
        --out "$RESULTS_DIR/with_symbols.json" \
        --timeout 30 \
        --diag 2>&1 | tee "$RESULTS_DIR/with_symbols.log"
echo ""

# ── Configuration 3: BM25 + symbols + codebase-memory-mcp graph ──
echo "━━━ [3/3] BM25 + symbols + MCP graph ━━━"
KNOCODE_MCP_ENABLED=true KNOCODE_SYMBOLS_ENABLED=true \
    python3 "$SCRIPT_DIR/metrics/retrieval.py" \
        --dataset "$SCRIPT_DIR/datasets/eshop_tasks.yaml" \
        --repo "$REPO" \
        --binary "$BINARY" \
        --out "$RESULTS_DIR/with_cbm_graph.json" \
        --timeout 30 \
        --diag 2>&1 | tee "$RESULTS_DIR/with_cbm_graph.log"
echo ""

# ── Summary Comparison ───────────────────────────────────────────
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║              3-WAY RETRIEVAL COMPARISON (48-task eShop)     ║"
echo "╠══════════════════════════════════════════════════════════════╣"

for config in baseline_bm25 with_symbols with_cbm_graph; do
    if [ -f "$RESULTS_DIR/${config}.json" ]; then
        label=$(echo "$config" | sed 's/_/ /g')
        summary=$(python3 -c "
import json, sys
with open('$RESULTS_DIR/${config}.json') as f:
    data = json.load(f)
s = data.get('summary', {})
print(f'  Recall@5:  {s.get(\"avg_recall@5\", 0):.4f}')
print(f'  Recall@10: {s.get(\"avg_recall@10\", 0):.4f}')
print(f'  MRR:       {s.get(\"avg_mrr\", 0):.4f}')
print(f'  Latency:   {s.get(\"avg_latency_ms\", 0):.0f}ms')
ma = s.get('miss_analysis', {})
if ma:
    print(f'  Misses:    {dict(ma)}')
" 2>/dev/null)
        echo "║  $label"
        echo "$summary"
        echo "║"
    fi
done

echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "Full results in: $RESULTS_DIR/"
echo "  baseline_bm25.json     — BM25 only (no symbols, no MCP)"
echo "  with_symbols.json      — BM25 + tree-sitter symbols (language-pack)"
echo "  with_cbm_graph.json    — BM25 + symbols + codebase-memory-mcp graph"
