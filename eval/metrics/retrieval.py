#!/usr/bin/env python3
"""
Retrieval quality metrics (TASK-006): Recall@5, Recall@10, MRR, tokens, latency, duplicate ratio.
Compares BuildContext code_context vs expected_files from repository_tasks.yaml
No mock fallback — fails hard if knocode preview times out so Recall@5/10 is honest.

Usage:
  python eval/metrics/retrieval.py --dataset eval/datasets/repository_tasks.yaml --k 5,10
"""

import argparse
import yaml
import time
import pathlib
import subprocess
import json
import sys
import io
import os

# Force UTF-8 output on Windows
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

# Preferred: structured `knocode preview --json` output (stable contract).
# Fallback: legacy regex scraping of human-readable stdout (kept for older binaries).
try:
    import json as _json  # already imported above; alias kept for clarity
except Exception:
    pass


def parse_json_output(out: str):
    """Parse `knocode preview --json` stdout; return ordered unique file paths or None."""
    try:
        data = json.loads(out)
    except Exception:
        return None
    if not isinstance(data, dict) or "files" not in data:
        return None
    paths = []
    for f in data["files"]:
        p = (f.get("path") or "").replace("\\", "/")
        if p and p not in paths:
            paths.append(p)
    return paths


def recall_at_k(expected, retrieved, k):
    if not expected:
        return 1.0
    topk = set(retrieved[:k])
    hits = len(set(expected) & topk)
    return hits / len(expected)


def mrr(expected, retrieved):
    for i, r in enumerate(retrieved, 1):
        if r in expected:
            return 1.0 / i
    return 0.0


def duplicate_ratio(retrieved):
    if not retrieved:
        return 0.0
    return 1.0 - (len(set(retrieved)) / len(retrieved))


def count_tokens_heuristic(text: str) -> int:
    return max(len(text) // 4, len(text.split()) if text else 0)


def read_file_preview(path: str, max_lines: int = 50) -> str:
    """Read first N lines of a file for reranking context."""
    try:
        with open(path, 'r', encoding='utf-8', errors='replace') as f:
            lines = [f.readline() for _ in range(max_lines)]
        return ''.join(lines)
    except Exception:
        return ''





def analyze_misses(expected_files, retrieved_files, query):
    """Classify why expected files were missed by retrieval (standalone Python analysis).
    
    This mirrors the Rust classify_misses function for use in the eval script.
    Returns a dict with miss categories and counts.
    """
    if not expected_files:
        return {}
    
    retrieved_set = set(retrieved_files)
    
    # Simple lexical analysis: check if expected file names contain query tokens
    stop_words = {
        'a','an','the','is','are','was','were','be','been','being',
        'have','has','had','do','does','did','will','would','could',
        'should','may','might','shall','can','to','of','in','for',
        'on','with','at','by','from','as','into','through','during',
        'before','after','above','below','between','and','but','or',
        'nor','not','so','yet','both','either','neither','each',
        'every','all','any','few','more','most','other','some',
        'such','no','only','own','same','than','too','very',
        'just','because','if','when','where','how','what','which',
        'who','whom','this','that','these','those',
    }
    query_tokens = [
        t.lower()
        for t in query.split()
        if len(t) >= 2 and t.lower() not in stop_words and t.isalnum()
    ]
    
    miss_counts = {}
    for expected in expected_files:
        # Check if retrieved (substring match)
        found = any(expected in r or r in expected for r in retrieved_set)
        if found:
            continue
        
        # Classify miss
        fname = expected.rsplit('/', 1)[-1].rsplit('\\', 1)[-1].lower()
        
        # Check lexical overlap with filename
        lexical_hits = sum(1 for t in query_tokens if t in fname)
        
        if lexical_hits == 0:
            miss_type = 'LEXICAL_MISS'
        elif lexical_hits < len(query_tokens) // 2:
            miss_type = 'QUERY_EXPANSION_MISS'
        else:
            miss_type = 'RANKED_TOO_LOW'
        
        miss_counts[miss_type] = miss_counts.get(miss_type, 0) + 1
    
    return miss_counts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", default="eval/datasets/repository_tasks.yaml")
    ap.add_argument("--k", default="5,10", help="Comma-separated k values for recall@k")
    ap.add_argument("--out", default="eval/results/evaluation.json")
    ap.add_argument("--timeout", type=int, default=15, help="Seconds before preview times out")
    ap.add_argument("--binary", default=None, help="Path to knocode binary")

    ap.add_argument("--repo", default=".", help="Repository root for file reads")
    ap.add_argument("--diag", action="store_true", help="Enable per-query retrieval diagnostic (classify misses)")
    args = ap.parse_args()

    ks = [int(x) for x in args.k.split(",")]
    data = yaml.safe_load(open(args.dataset))
    if not isinstance(data, list):
        print(f"ERROR: dataset {args.dataset} did not parse as list (got {type(data)})", file=sys.stderr)
        sys.exit(1)

    print(f"Loaded {len(data)} tasks from {args.dataset}")


    results = []
    total_mrr = 0.0
    total_latency_ms = 0.0
    recall_sums = {k: 0.0 for k in ks}

    for task in data[:50]:
        expected = task.get("expected_files", [])
        task_str = str(task.get("task") or "")
        retrieved = []
        latency_ms = 0
        t0 = time.time()
        try:
            task_str = str(task_str or "")
            if args.binary:
                binary = args.binary
            else:
                # Probe repo-local + shared cargo target dirs, pick the NEWEST
                # (target-dir in ~/.cargo/config.toml can redirect builds, and a
                # stale repo-local binary must not shadow a fresh shared one).
                home = os.path.expanduser("~")
                candidates = [
                    os.path.join("target", "release", "knocode.exe"),
                    os.path.join("target", "release", "knocode"),
                    os.path.join(home, ".cargo", "target", "release", "knocode.exe"),
                    os.path.join(home, ".cargo", "target", "release", "knocode"),
                ]
                existing = [c for c in candidates if os.path.exists(c)]
                if existing:
                    binary = max(existing, key=os.path.getmtime)
                else:
                    binary = os.path.join("target", "release", "knocode.exe")
            cmd = [binary, "preview", task_str, "--json"]
            if args.diag and expected:
                cmd.extend(["--diag", "--expected-files", ",".join(expected)])
            proc = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=args.timeout,
                cwd=args.repo,
            )
            latency_ms = int((time.time() - t0) * 1000)
            if proc.returncode != 0 and not proc.stdout:
                raise RuntimeError(
                    f"knocode preview failed (rc={proc.returncode}) stderr={(proc.stderr or '')[:500]}"
                )
            out = (proc.stdout or "") + (proc.stderr or "")
            # Preferred: structured --json contract (stable across platforms/formats)
            retrieved = parse_json_output(proc.stdout or "") or []
            if not retrieved:
                # Legacy fallback: only accept real file-path anchors:
                # // <path with separator>.<ext>[:line]
                import re
                path_anchor = re.compile(
                    r"^//[ ./]*([^\s:`\"]+\.(?:cs|cshtml|html|razor|json|ts|tsx|js|jsx|md|yml|yaml|cpp|h|go|rs|py))\b",
                    re.IGNORECASE,
                )
                for line in out.splitlines():
                    m = path_anchor.match(line.strip().lstrip("* "))
                    if not m:
                        continue
                    p = m.group(1).strip().replace("\\", "/")
                    if p and p not in retrieved:
                        retrieved.append(p)
        except subprocess.TimeoutExpired:
            print(
                f"ERROR: knocode preview timeout ({args.timeout}s) for task '{task_str[:60]}'",
                file=sys.stderr,
            )
            sys.exit(2)
        except FileNotFoundError as e:
            print(f"ERROR: binary not found: {e}", file=sys.stderr)
            sys.exit(2)
        except RuntimeError:
            raise
        except Exception as e:
            print(f"ERROR: preview exception for '{task_str[:60]}': {e}", file=sys.stderr)
            sys.exit(2)

        m = mrr(expected, retrieved)
        total_mrr += m
        total_latency_ms += latency_ms
        recs = {}
        for k in ks:
            r = recall_at_k(expected, retrieved, k)
            recs[f"recall@{k}"] = round(r, 4)
            recall_sums[k] += r
        dup = duplicate_ratio(retrieved)
        tokens = count_tokens_heuristic("\n".join(retrieved))

        # Analyze misses (standalone Python classification)
        miss_analysis = analyze_misses(expected, retrieved, task_str) if expected else {}
        total_misses = sum(miss_analysis.values())
        
        diag_label = f" misses={total_misses}" if miss_analysis else ""
        print(
            f"task={task_str[:40]:40} recall@5={recs.get('recall@5', 0):.2f} recall@10={recs.get('recall@10', 0):.2f} mrr={m:.2f} latency={latency_ms}ms{diag_label}"
        )
        if miss_analysis and args.diag:
            for miss_type, count in sorted(miss_analysis.items()):
                print(f"    [{miss_type}] {count} files")
        results.append(
            {
                "task": task_str,
                "category": task.get("category", ""),
                "expected_files": expected,
                "retrieved": retrieved,
                "mrr": round(m, 4),
                "latency_ms": latency_ms,
                "duplicate_ratio": round(dup, 4),
                "tokens_heuristic": tokens,
                "miss_analysis": miss_analysis if miss_analysis else None,
                **recs,
            }
        )

    n = len(results) or 1
    summary = {
        "total_tasks": len(results),
        "avg_mrr": round(total_mrr / n, 4),
        "avg_latency_ms": round(total_latency_ms / n, 1),
        "avg_duplicate_ratio": round(sum(r["duplicate_ratio"] for r in results) / n, 4) if results else 0,
    }
    for k in ks:
        summary[f"avg_recall@{k}"] = round(recall_sums[k] / n, 4)

    # Aggregate miss analysis
    if args.diag:
        total_miss_counts = {}
        for r in results:
            ma = r.get("miss_analysis")
            if ma:
                for miss_type, count in ma.items():
                    total_miss_counts[miss_type] = total_miss_counts.get(miss_type, 0) + count
        if total_miss_counts:
            summary["miss_analysis"] = total_miss_counts
            print(f"\nMiss analysis across {len(results)} tasks:")
            for miss_type, count in sorted(total_miss_counts.items()):
                print(f"  {miss_type}: {count} files")

    out_path = pathlib.Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {"summary": summary, "ks": ks, "results": results}
    out_path.write_text(json.dumps(payload, indent=2))
    print(f"\nSummary: {json.dumps(summary, indent=2)}")
    print(f"Wrote {len(results)} results to {out_path}")


if __name__ == "__main__":
    main()
