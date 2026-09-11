#!/usr/bin/env python3
"""Compare codebase-memory-mcp search_graph vs full local pipeline (knocode preview).

MCP search_graph uses dependency graph + BM25 for natural language queries.
Local pipeline uses BM25 + tree-sitter symbols.
"""

import argparse
import json
import subprocess
import sys
import io
import time
import yaml
import os
import pathlib

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding="utf-8", errors="replace")

CBM_BIN = os.path.expanduser("~/.knocode/bin/codebase-memory-mcp.exe")
if not os.path.exists(CBM_BIN):
    # legacy ~/bin fallback
    _legacy = os.path.expanduser("~/bin/codebase-memory-mcp.exe")
    CBM_BIN = _legacy if os.path.exists(_legacy) else "codebase-memory-mcp"

EXTS = ("cs", "cshtml", "razor", "json", "ts", "tsx", "js", "jsx", "md", "yml",
        "yaml", "cpp", "h", "go", "rs", "py", "java", "kt", "rb", "php", "swift", "scala")


def _has_ext(path):
    for ext in EXTS:
        if path.endswith("." + ext):
            return True
    return False


def recall_at_k(expected, retrieved, k):
    if not expected:
        return 1.0
    return len(set(expected) & set(retrieved[:k])) / len(expected)


def mrr(expected, retrieved):
    for i, r in enumerate(retrieved, 1):
        if r in expected:
            return 1.0 / i
    return 0.0


def search_mcp(query, project, top_k=50):
    """Search using codebase-memory-mcp CLI search_graph (graph + BM25)."""
    try:
        args = [
            CBM_BIN,
            "cli",
            "search_graph",
            json.dumps({"query": query, "project": project, "top_k": top_k}),
            "--json",
        ]
        proc = subprocess.run(
            args,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
        out = proc.stdout + proc.stderr
        for line in out.splitlines():
            line = line.strip()
            if line.startswith("{"):
                try:
                    data = json.loads(line)
                    text = data.get("content", [{}])[0].get("text", "")
                    files = []
                    for l in text.splitlines():
                        l = l.strip()
                        # graph format: "QN.Label src/path/file.ext line-range rank"
                        parts = l.split()
                        for part in parts:
                            p = part.replace("\\", "/")
                            if "/" in p and _has_ext(p):
                                if p not in files:
                                    files.append(p)
                                break
                    return files[:top_k]
                except Exception:
                    pass
        return []
    except Exception as e:
        print("  MCP error: {}".format(e), file=sys.stderr)
        return []


def search_local(query, binary, repo, timeout=15):
    """Search using knocode preview (full local pipeline)."""
    try:
        proc = subprocess.run(
            [binary, "preview", query],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            cwd=repo,
        )
        out = proc.stdout + proc.stderr
        files = []
        for line in out.splitlines():
            stripped = line.strip()
            if stripped.startswith("//"):
                rest = stripped[2:].strip()
                for token in rest.split():
                    token = token.replace("\\", "/")
                    token = token.split(":")[0]
                    if "/" in token and _has_ext(token):
                        if token not in files:
                            files.append(token)
                        break
        return files
    except Exception as e:
        print("  Local error: {}".format(e), file=sys.stderr)
        return []


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", default="eval/datasets/eshop_tasks.yaml")
    ap.add_argument("--repo", default="C:/LeonRepository/eShopOnWeb")
    ap.add_argument(
        "--binary", default="C:/LeonRepository/knocode/target/release/knocode.exe"
    )
    ap.add_argument("--out", default="eval/results/mcp_vs_local.json")
    ap.add_argument("--timeout", type=int, default=15)
    args = ap.parse_args()

    project = "C-LeonRepository-eShopOnWeb"
    data = yaml.safe_load(open(args.dataset))
    print("Loaded {} tasks".format(len(data)))

    results = []
    for i, task in enumerate(data[:50]):
        expected = task.get("expected_files", [])
        query = str(task.get("task", ""))
        print(
            "[{}/{}] {}...".format(i + 1, len(data), query[:50]),
            end=" ",
            flush=True,
        )

        # MCP search_graph (graph + BM25)
        t0 = time.time()
        mcp_files = search_mcp(query, project)
        mcp_lat = int((time.time() - t0) * 1000)

        # Local pipeline (BM25 + symbols)
        t0 = time.time()
        local_files = search_local(query, args.binary, args.repo, args.timeout)
        local_lat = int((time.time() - t0) * 1000)

        m = {
            "task": query,
            "expected": expected,
            "mcp_files": mcp_files,
            "mcp_latency": mcp_lat,
            "local_files": local_files,
            "local_latency": local_lat,
        }
        for k in [5, 10]:
            m["mcp_r@{}".format(k)] = round(recall_at_k(expected, mcp_files, k), 4)
            m["local_r@{}".format(k)] = round(
                recall_at_k(expected, local_files, k), 4
            )
        m["mcp_mrr"] = round(mrr(expected, mcp_files), 4)
        m["local_mrr"] = round(mrr(expected, local_files), 4)

        print(
            "MCP: r@5={:.2f} {}ms | Local: r@5={:.2f} {}ms".format(
                m["mcp_r@5"], mcp_lat, m["local_r@5"], local_lat
            )
        )
        results.append(m)

    # Aggregate
    n = len(results) or 1
    summary = {}
    for prefix in ["mcp", "local"]:
        summary["{}_r@5".format(prefix)] = round(
            sum(r["{}_r@5".format(prefix)] for r in results) / n, 4
        )
        summary["{}_r@10".format(prefix)] = round(
            sum(r["{}_r@10".format(prefix)] for r in results) / n, 4
        )
        summary["{}_mrr".format(prefix)] = round(
            sum(r["{}_mrr".format(prefix)] for r in results) / n, 4
        )
        summary["{}_latency".format(prefix)] = round(
            sum(r["{}_latency".format(prefix)] for r in results) / n, 0
        )

    out_path = pathlib.Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(
        json.dumps({"summary": summary, "results": results}, indent=2)
    )

    # Print table
    print()
    hdr = "+----------------------------------------------+----------+----------+---------+----------+"
    print(hdr)
    print("| Config                                       |     R@5  |    R@10  |     MRR |  Latency |")
    print(hdr)
    print(
        "| codebase-memory-mcp (graph+BM25)             | {:>8.4f} | {:>8.4f} | {:>7.4f} | {:>6.0f}ms |".format(
            summary["mcp_r@5"],
            summary["mcp_r@10"],
            summary["mcp_mrr"],
            summary["mcp_latency"],
        )
    )
    print(
        "| Full Local (BM25 + tree-sitter symbols)      | {:>8.4f} | {:>8.4f} | {:>7.4f} | {:>6.0f}ms |".format(
            summary["local_r@5"],
            summary["local_r@10"],
            summary["local_mrr"],
            summary["local_latency"],
        )
    )
    print(hdr)
    print("\nResults: {}".format(args.out))


if __name__ == "__main__":
    main()
