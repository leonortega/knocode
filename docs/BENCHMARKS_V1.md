# 🚀 Knocode Retrieval Engine — Benchmark Report

> **Engine:** v0.9.11 (`--release`) · **Method:** 50 hard queries per repo vs `grep -rE` baseline · **Repos:** knocode (163 files), Mattermost (9,850), DefinitelyTyped (53,828)
> All current-engine numbers below were re-verified 2026-09-11 (final lock-in run; see "Version progression" for the v0.9.9 baselines).

---

## 🎯 TL;DR — Knocode vs Grep

| Repo (files) | grep p50 | knocode p50 | **Speedup** | Recall of grep hits | **Novelty** (grep can't find) | avg MRR |
|---|---|---|---|---|---|---|
| knocode (163) | ~500 ms | 1–2 ms | **152–228×** | **100%** (grep-only: 0) | 86.7% | — |
| Mattermost (9,850) | ~800 ms | 6 ms | **133×** | 13.1% | 53.0% | 0.5948 |
| DefinitelyTyped (53,828) | ~5,100 ms | 9 ms | **218×** | 17.2% | 36.3% | — |

**The four claims that matter:**

1. **Speed:** knocode answers in **1–9 ms p50 across three orders of magnitude of repo size** — grep needs 0.5–5 s. That gap is what makes per-keystroke retrieval viable and grep unusable for real-time AI coding.
2. **Novelty:** **36–87% of what knocode returns is invisible to grep** — documentation, tests, related components, config found through semantic understanding, not literal patterns.
3. **Coverage where it counts:** on small repos (≤500 files, now the production default) knocode returns **every file grep finds, on every query** (recall 100%, grep-only 0) *plus* the semantically related set — at a context-pack cost of just **+5.8% tokens over the old 20-file pack** for 2.6× the files.
4. **Ranking quality:** on the 50-task golden eval, MRR improved **0.245 → 0.697** across this cycle — the expected file is now typically the *first* result, not just somewhere in the list (grep returns unordered dumps).

*Speedup ratios vary run-to-run with the grep subprocess (cache/AV state); knocode-side latencies are the stable quantity.*

---

## 🧩 What We're Measuring

| Metric | Meaning | Why it matters |
|---|---|---|
| Retrieval latency | Engine query time | Real-time AI coding needs single-digit ms |
| Grep latency | `grep -rE` baseline | What developers use today |
| Speedup | grep time ÷ engine time | The headline comparison |
| Recall | Fraction of grep's hits we also return | Are we missing obvious matches? |
| Precision | Fraction of our results that are grep hits | Are we returning junk? (intentionally low — we return the *useful* set, not *all* files) |
| Novelty | Fraction of our results grep *cannot* find | The semantic-understanding dividend |
| MRR | Mean reciprocal rank of the expected file | Is the right file ranked first? |

Benchmarks score **basename-set** overlap vs grep; the golden eval (below) measures true ranking quality. Set metrics cap what they can show — MRR closes that blind spot.

---

## 🏗️ Results by Repo (current engine, v0.9.11)

### Small repo — knocode (163 files), k=50 auto-tuned default

```
┌─────────────────────────────┬──────────────┐
│  Retrieval avg              │  ~3 ms       │
│  Grep avg                   │  500–740 ms  │
│  ⚡ Speedup                 │  152–228×    │
│  Recall (ret ∩ grep / grep) │  100.0%      │  ← every grep hit, every query
│  Grep-only                  │  0           │
│  Novelty                    │  86.7%       │  ← most of what we return is semantic
└─────────────────────────────┴──────────────┘
```

The index is exhaustive at this scale — only the result-set cut ever limited recall. Since 2026-09-11 the small-repo arm auto-raises k 20→50 (with pool co-scaled ≥ 4×k), reaching **100% recall at default config**. `BuildContext` (full pack, criterion, 100 samples): **13.82 ms** — target p95 < 50 ms ✅.

Top wins over grep (per-query novelty): "what is the architecture of the retrieval engine" → 47 files grep can't surface; "which files are excluded from indexing" → 46; "find all test files that test the retrieval engine" → 46.

### Mattermost — 9,850 Go + React files (warm index)

```
┌─────────────────────────────┬──────────────┐
│  Retrieval avg / p50 / p95  │  6 / 6 / 8 ms│
│  Grep avg                   │  ~800 ms     │
│  ⚡ Speedup                 │  133×        │
│  Recall                     │  13.1%       │
│  Precision                  │  32.7%       │
│  Novelty                    │  53.0%       │  ← half our results are novel
│  avg MRR (basename)         │  0.5948      │
└─────────────────────────────┴──────────────┘
```

Cross-layer queries show the semantic gap best: "how to create a new React component" → 50 novel files (docs, templates, examples); "how to add a new API endpoint" → 47 (REST docs, endpoint patterns). All 50 queries complete under 10 ms except one legacy debugging outlier — the v0.9.9 216 ms Tantivy-panic fallback no longer reproduces (zero fallbacks in the final run).

### DefinitelyTyped — 53,828 TypeScript files

```
┌─────────────────────────────┬──────────────┐
│  Retrieval avg / p50 / p95  │  17 / 9 / 14 ms│
│  Grep avg                   │  ~3,700 ms   │
│  ⚡ Speedup                 │  218×        │
│  Recall                     │  17.2%       │
│  Precision                  │  9.1%        │
│  Novelty                    │  36.3%       │
└─────────────────────────────┴──────────────┘
```

At 53k files, p95 is 14 ms with **no slow-query outliers** — the v0.9.9 446 ms debugging average (Tantivy phrase-query panic → ripgrep fallback) is gone entirely. Top wins: "find all utility type definitions (Partial, Pick, Omit)" → 84 files grep can't surface; "find all enum definitions with string values" → 43; "why does TypeScript complain about this conditional type" → 36.

---

## 📈 Version Progression (v0.9.9 → v0.9.11)

Same configs, same result-set sizes — every quality metric held or improved while the engine got 2–7.5× faster. The speedup jumps are **engine-side** (panic-fallback outliers eliminated, ranking pipeline tightened), not a slower grep baseline (grep actually got slightly faster between runs).

| Bench | Metric | v0.9.9 | v0.9.11 | Δ |
|---|---|---|---|---|
| DT (k=50) | Retrieval avg latency | 128 ms | 17 ms | **7.5× faster** |
| DT (k=50) | Speedup | 36.5× | 217.8× | **~6× better** |
| DT (k=50) | Recall / Precision | 17.3% / 9.3% | 17.2% / 9.1% | ≈ (noise) |
| Mattermost (k=50) | Retrieval avg latency | 11 ms | 6 ms | **~2× faster** |
| Mattermost (k=50) | Speedup | 67.2× | 133.1× | **~2× better** |
| Mattermost (k=50) | Recall / Precision | 13.0% / 32.2% | 13.1% / 32.7% | ≈ / +0.5pp |
| Golden 50-task eval | **MRR** | 0.245 | **0.697** | **+0.45** |
| Golden 50-task eval | avg latency (incl. spawn) | 176 ms | 144–159 ms | **−10/−18%** |
| Golden 50-task eval | R@5 / R@10 | 0.573 / 0.697 | **0.613 / 0.727** | +0.04 / +0.03 |
| BuildContext (criterion) | mean | 15.2 ms | **13.82 ms** | −8.5% |

**What drove the MRR jump (ranking refactor, 2026-09-10):** weighted query expansion (original query at full BM25 weight, synonyms gap-fill at 0.5×); segment-based path matching (kills false boosts like "test" matching `latest.rs`); `is_test_query` whole-token fix; deterministic tie-breaks; canonical synonym table (~28 entries) shared by storage and context with parity tests; zero-alloc `symbol_boost`.

**Token comparison (same 50 tasks, honest accounting):** baseline (no knocode) **1,807** tokens vs knocode **4,537** — the pack injects 50 curated files (auto-tuned) with evidence-scaled snippet windows holding it at **+5.8%** over the true 19-file old path (4,287). Knocode *cuts* tool tokens 600 → ~85 (−86%) by answering before the agent has to explore. Earlier readings (2,167 / 4,233) accidentally measured the JSON envelope, not the pack; the harness now reads `token_usage.total`.

---

## 🧪 Ranking Quality — the Golden Eval (50 tasks, this repo)

| Metric | Value |
|---|---|
| **MRR** | **0.6965** (was 0.245 pre-refactor) |
| Recall@5 / Recall@10 | 0.613 / 0.727 |
| avg latency (incl. process spawn) | 144–159 ms |
| duplicate ratio | 0.0 |

MRR ≈ 0.70 means the expected file is typically **rank #1**. Historical zero-recall analysis (15/50 tasks before the dataset refresh): 4 were dataset bugs (expectations pointed at deleted files), 7 mixed stale + live files, 4 pure misses; of the live ones, 9 were LEXICAL_MISS (expected file contains ≈no query terms — expectations describe *changes to be made*, not existing content). The dataset was refreshed 2026-09-07 accordingly.

---

## 🎛️ Runtime Knobs — What Moves and What Doesn't (Mattermost, 9,850 files)

Full option matrix (2026-09-10): every runtime knob was swept; **all left ranking order byte-identical** (avg MRR 0.5948 across 13 configurations) — knobs trade only latency and result-set shape.

| Knob | Finding |
|---|---|
| Symbols off | Zero visible delta (they cost 49% of *cold* index time, ~0 warm — keep) |
| Graph forced | Identical quality, 2.2× slower warm, 33.5 s one-time cold build → the 5k-file size gate is correct; `KNOCODE_GRAPH_MAX_FILES` is the opt-in hatch |
| `KNOCODE_CANDIDATE_K` 500/1000 | Strictly worse at this scale (−7/−15 overlap, +2/+6 ms); pool 200 saturates |
| `KNOCODE_DOCS_RESERVE` 0/10 | ±1pp precision/novelty trade; default 2 within ±0.2pp of both extremes |
| Result-set size k=100/150 | The only recall lever here (+0.17pp/file; precision dilutes; MRR flat) |

**Defaults are simultaneously the fastest and tied-best configuration** — confirmed across all 13 runs.

### Result-set sweep across benches (knob: `KNOCODE_BENCH_MAX_FILES`)

| Bench | Baseline k | Recall → bigger k | Note |
|---|---|---|---|
| knocode (163 files) | 20 | 88.0% → **100%** @ 50 | index exhaustive; cut-off was the only limit — now the production default |
| DT (53,828) | 50 | 17.2% → 18.3% @ 100 | recall ceiling is the relevance model, not the cut |
| Mattermost (9,850) | 50 | 13.1% → 16.4% @ 100 | MRR flat 0.5948 → 0.5950 |
| bench_components | 50 | CandK +0.0% → **+44.4%** @ 100; QE +4.8% → +18.2% | pool size matters once the cut passes the stabilized top region — **interpret pool and cut together** |

Decision: keep k=20/50 defaults (precision-first for LLM-consumed context); an "exhaustive mode" would raise `max_files` **and** `candidate_k` together.

---

## ⚙️ Process Reliability Fixes (this cycle)

| Fix | Impact |
|---|---|
| **Index heal loop** (`knocode-repo-intel`) | Heal runs persisted symbols only for *changed* files, so a symbol-less DB re-healed **forever** (full re-extraction every run, ~14 s at 9.8k files). Now one-shot; verified 36,861 symbols persisted → next run warm in 480 ms. `IndexStats` reports run mode (WARM/INCREMENTAL/HEAL) + per-phase timings, so "why was indexing slow?" is visible in every log. |
| **Tokenizer hot-path bug** (`knocode-context`) | `count_tokens` rebuilt the ~100k-entry BPE tokenizer **per call** (per *line* inside the truncation loop): a single over-budget `preview` cost **25.3 s**. Now built once (`OnceLock`), warmed at `ContextEngine::new` (~43 ms at startup), guarded by a regression test (deterministic static check + timing ceiling). Same query: **25,305 → 80 ms**. |
| **Small-repo auto-tune + explicit pins** (`policy.rs`) | doc_count ≤ 500 → k 20→50 (pool co-scaled ≥ 4×k) at default config; a `max_files_explicit` flag makes `--max-files` / `KNOCODE_MAX_FILES` / `KNOCODE_BENCH_MAX_FILES` pins **never** overridden (previously `KNOCODE_MAX_FILES=20` was silently raised to 50). Verified: default → 50; `=20` → 20; `=15` → 15; bench pinned-20 reproduces the historical 88% row exactly. |
| **Pack-size re-tune** (`scaled_snippet_window`) | Beyond 20 evidence entries the per-file snippet window scales down (6-line floor), holding the 50-file pack at **+5.8%** vs the old path with file membership — and recall — untouched. |
| `preview --json` | Stable machine-readable contract (paths, provenance, token usage, retrieval stats); both eval harnesses consume it. |

---

## 🏛️ How the Engine Beats Grep (architecture in one page)

```
Query "how to add error handling"
  → Intent detection (<1 ms)        procedural / debugging / structural …
  → Query expansion (<1 ms)         + synonyms (canonical table, weighted 0.5×)
  → Candidate retrieval (~5 ms)     Tantivy BM25 → 200-ranked pool
                                    (panic → graceful ripgrep fallback)
  → Ranking (~1 ms)                 field/class/path weights, symbol boost,
                                    deterministic tie-breaks → top-k files
```

| User intent | Grep finds | Knocode finds |
|---|---|---|
| "how to add error handling" | files with literal "error handling" | try/catch patterns, error types, **docs** |
| "why does the auth fail" | literal "auth" + "fail" | auth middleware, session handling, permission checks |
| "find all API endpoints" | literal "API" + "endpoint" | route definitions, handler registrations, API docs |

Graceful degradation is deliberate: when Tantivy panics on phrase queries, it falls back to ripgrep instead of crashing; ast-grep returns errors on ambiguous patterns. Related files beat zero results for an AI assistant.

---

## 🐛 Known Issues & Fixes

| # | Issue | Status |
|---|---|---|
| 1 | Tantivy phrase-query panics crashed queries | ✅ `catch_unwind` → ripgrep fallback; zero outliers on 53k files (final run) |
| 2 | ast-grep `MultipleNode` panics on ambiguous patterns | ✅ `Pattern::try_new()`; zero panics on TS patterns |
| 3 | Benches missing index build | ✅ self-contained (`index_repository()` before queries) |
| 4 | Structural-query recall on DT ("find all X") | ⚠️ structural mode helps; DT's flat 53k structure stays hard |
| 5 | Graph boost neutral on every tested repo | ⚠️ gated at 5k files; future value is cross-layer queries + cache invalidation keyed on index generation |

Open roadmap: cross-layer graph value (Go handler ↔ React component) for repos that need it; cache warming for sub-5 ms repeated queries.

---

## 🔗 Reproducing

```bash
# Build once
cargo build --release -p knocode-cli

# Golden 50-task eval (this repo) — MRR / recall / latency
python eval/metrics/retrieval.py --dataset eval/datasets/repository_tasks.yaml

# Baseline-vs-knocode token comparison
python eval/baseline/run.py

# Grep-comparison benches (result-set knob works on all: KNOCODE_BENCH_MAX_FILES=100 …)
cargo test --release -p knocode-context --lib -- --ignored bench_retrieval --nocapture   # knocode repo
cargo test --release -p knocode-context --lib -- --ignored bench_mattermost --nocapture  # 9.8k files
cargo test --release -p knocode-context --lib -- --ignored bench_dt --nocapture          # 53.8k files
cargo test --release -p knocode-context --lib -- --ignored bench_components --nocapture  # ablations

# Full-pack latency (criterion, 100 samples)
cargo bench -p knocode-bench --bench context_bench
```

**Requirements:** DT at `C:/tmp/DefinitelyTyped-master`, Mattermost at `C:/tmp/mattermost-master`; **always `--release`** for latency numbers. Env pins (`KNOCODE_BENCH_MAX_FILES`, `KNOCODE_CANDIDATE_K`, `KNOCODE_DOCS_RESERVE`, `KNOCODE_GRAPH_MAX_FILES`, `KNOCODE_SYMBOLS_ENABLED`) are explicit and never auto-overridden.

Dependencies audited 2026-09-04 — all current (tantivy 0.26.1, rusqlite 0.40.2, tokio 1.53.1, tiktoken-rs 0.12.0, ast-grep-core 0.45.2, tree-sitter-language-pack 1.16.1).

---

*Knocode Benchmarks v1 — consolidated 2026-09-11 (engine v0.9.11). Supersedes the accreted per-fix notes; version history is preserved in the table above and in git.*
