#!/usr/bin/env python3
"""Compare BM25 vs MCP retrieval on failing tasks from the 50-task eval."""
import subprocess, json, sys, io, yaml, time, os

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

def run_bm25(query, timeout=30):
    binary = os.path.join("target", "release", "knocode.exe")
    if not os.path.exists(binary):
        binary = os.path.join("target", "release", "knocode")
    try:
        proc = subprocess.run(
            [binary, "preview", query],
            capture_output=True, text=True, encoding='utf-8', errors='replace',
            timeout=timeout, cwd=r"C:\LeonRepository\knocode"
        )
        files = []
        for line in (proc.stdout or "").splitlines():
            s = line.strip()
            if s.startswith("// ") and ":" in s:
                p = s[3:].split(":")[0].strip().replace("\\", "/")
                if p and p not in files:
                    files.append(p)
            elif s.startswith("// ") and "/" in s:
                p = s[3:].strip().split()[0].replace("\\", "/")
                if p and p not in files:
                    files.append(p)
        return files
    except Exception as e:
        return []

def run_mcp(query, timeout=60):
    try:
        cmd = f'npx codebase-memory-mcp cli search_code --pattern "{query}" --project knocode --file-pattern "*.rs" --limit 10 --json'
        proc = subprocess.run(
            cmd,
            capture_output=True, text=True, encoding='utf-8', errors='replace',
            timeout=timeout, cwd=r"C:\LeonRepository\knocode", shell=True
        )
        out = proc.stdout or ""
        files = []
        for line in out.splitlines():
            line = line.strip()
            if not line or not line.startswith("{"):
                continue
            try:
                data = json.loads(line)
                text = data.get("content", [{}])[0].get("text", "")
                for l in text.splitlines():
                    l = l.strip()
                    if not l or l.startswith("results:") or l.startswith("dirs:") or l.startswith("total") or l.startswith("raw:") or l.startswith("raw_match"):
                        continue
                    parts = l.split()
                    if len(parts) >= 3:
                        fp = parts[2].replace("\\", "/")
                        if fp.endswith((".rs", ".ts", ".js", ".py", ".cs", ".go")) and fp not in files:
                            files.append(fp)
                return files[:10]
            except json.JSONDecodeError:
                continue
        return []
    except Exception as e:
        return []

def recall(expected, retrieved, k=5):
    if not expected:
        return 1.0
    topk = set(retrieved[:k])
    return len(set(expected) & topk) / len(expected)

# Load eval results
with open("eval/results/evaluation.json") as f:
    eval_data = json.load(f)

# Find failing tasks (recall@5 == 0)
failing = [r for r in eval_data["results"] if r.get("recall@5", 0) == 0.0]
print(f"Found {len(failing)} failing tasks out of {len(eval_data['results'])}")
print(f"{'='*80}")

mcp_hits = 0
mcp_total = 0
results = []

for r in failing:
    task = r["task"]
    expected = r["expected_files"]
    bm25_retrieved = r["retrieved"]
    
    t0 = time.time()
    mcp_retrieved = run_mcp(task)
    mcp_latency = int((time.time() - t0) * 1000)
    
    bm25_recall = recall(expected, bm25_retrieved)
    mcp_recall = recall(expected, mcp_retrieved)
    
    improved = mcp_recall > bm25_recall
    if improved:
        mcp_hits += 1
    mcp_total += 1
    
    status = "IMPROVED" if improved else ("SAME" if mcp_recall == bm25_recall else "WORSE")
    print(f"task={task[:50]:50} bm25={bm25_recall:.2f} mcp={mcp_recall:.2f} {status} ({mcp_latency}ms)")
    if mcp_retrieved:
        print(f"  mcp_files: {mcp_retrieved[:3]}")
    
    results.append({
        "task": task, "expected": expected,
        "bm25_files": bm25_retrieved, "mcp_files": mcp_retrieved,
        "bm25_recall": bm25_recall, "mcp_recall": mcp_recall,
        "mcp_latency_ms": mcp_latency, "improved": improved
    })

print(f"{'='*80}")
print(f"MCP improved {mcp_hits}/{mcp_total} failing tasks")
print(f"Improvement rate: {mcp_hits/mcp_total*100:.1f}%" if mcp_total else "N/A")

# Save results
with open("eval/results/mcp_comparison.json", "w") as f:
    json.dump({"summary": {"improved": mcp_hits, "total": mcp_total, "rate": mcp_hits/mcp_total if mcp_total else 0}, "results": results}, f, indent=2)
