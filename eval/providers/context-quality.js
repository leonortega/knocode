/**
 * Context Quality Evaluation Provider for Promptfoo
 *
 * Exports a provider class with id() and callApi() methods.
 *
 * Primary path: real BuildContext via the daemon's HTTP hook API —
 * POST ${KNOCODE_DAEMON_URL:-http://127.0.0.1:9527}/hook with a
 * PreGeneration/MessageRewrite request (the same endpoint production
 * agents use). The daemon scopes retrieval to this repo via
 * `repository_path` (TASK-036); vars.files_mentioned maps to
 * context_hints.files_mentioned.
 *
 * Mock path: tests whose vars describe simulated inputs the real API
 * does not accept (knowledge_entries, large_file_count) — and any
 * request the daemon fails to answer — are served by mockContextEngine,
 * which mirrors the real ContextPack shape (docs_context, code_context,
 * token_usage, metadata). Output.source records which path produced it:
 * "http" | "mock" (mock-contract test) | "mock-fallback" (daemon down).
 */

const path = require("path");

const DAEMON_URL = process.env.KNOCODE_DAEMON_URL || "http://127.0.0.1:9527";
// Daemon scopes retrieval to the agent's workspace root — default to this repo.
const REPO_ROOT = process.env.KNOCODE_REPO_PATH || path.resolve(__dirname, "..", "..");
const TIMEOUT_MS = parseInt(process.env.KNOCODE_EVAL_TIMEOUT_MS, 10) || 5000;

function splitList(s) {
  return s ? String(s).split(",").map((x) => x.trim()).filter(Boolean) : [];
}

/** Deterministic mock of the Knocode context engine (real ContextPack shape). */
function mockContextEngine(vars) {
  const task = vars.task || "";
  const max_tokens = parseInt(vars.max_tokens, 10) || 12000;

  const files = splitList(vars.files_mentioned);
  const filler = Math.max(0, parseInt(vars.large_file_count, 10) || 0);
  const knowledge = splitList(vars.knowledge_entries)
    .map((entry) => {
      const idx = entry.indexOf(":");
      return idx === -1
        ? { key: entry, value: "" }
        : { key: entry.slice(0, idx).trim(), value: entry.slice(idx + 1).trim() };
    })
    .filter((k) => k.key);

  const docs_context = knowledge.map((k) => `// ${k.key}: ${k.value}`).join("\n");
  const allFiles = files.concat(Array.from({ length: filler }, (_, i) => `generated/file_${i}.rs`));
  let code_context = allFiles.map((f) => `// ${f}\n// [file content]`).join("\n\n");

  // Enforce the token budget (~4 chars/token); code_context is truncated first
  const docs_tokens = Math.floor(docs_context.length / 4);
  let code_tokens = Math.floor(code_context.length / 4);
  if (docs_tokens + code_tokens > max_tokens) {
    code_context = code_context.slice(0, Math.max(0, max_tokens - docs_tokens) * 4);
    code_tokens = Math.floor(code_context.length / 4);
  }
  const total_tokens = docs_tokens + code_tokens;

  return {
    docs_context,
    code_context,
    token_usage: {
      total_tokens,
      budget_remaining: Math.max(0, max_tokens - total_tokens),
      by_source: { docs_context: docs_tokens, code_context: code_tokens },
    },
    metadata: { task_length: task.length, files: allFiles.length },
  };
}

module.exports = class ContextQualityProvider {
  id() {
    return "context-quality";
  }

  label = "Context Quality (daemon HTTP /hook, mock fallback)";

  async callApi(prompt, context) {
    const vars = context?.vars || {};
    // Vars only the mock contract understands never go to the daemon.
    const mockContract = vars.knowledge_entries || vars.large_file_count;
    if (!mockContract) {
      try {
        return await this.callDaemon(prompt, vars);
      } catch (e) {
        console.warn(`[context-quality] daemon unavailable (${e.message}); mock fallback`);
      }
    }
    const result = mockContextEngine(vars);
    result.source = mockContract ? "mock" : "mock-fallback";
    return { output: JSON.stringify(result, null, 2) };
  }

  /** Real BuildContext over the daemon HTTP hook API. Throws on any failure. */
  async callDaemon(prompt, vars) {
    const payload = {
      type: "MessageRewrite",
      session_id: "eval",
      message: prompt,
      repository_path: REPO_ROOT,
    };
    const files = splitList(vars.files_mentioned);
    if (files.length || vars.language) {
      payload.context_hints = {};
      if (files.length) payload.context_hints.files_mentioned = files;
      if (vars.language) payload.context_hints.language = vars.language;
    }

    const res = await fetch(`${DAEMON_URL}/hook`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ hook_type: "PreGeneration", payload }),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    const json = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(`daemon ${res.status}: ${json.error || "request failed"}`);

    const p = json.payload || {};
    if (p.type === "RewrittenMessage") {
      const pack = p.context_pack || {};
      return {
        output: JSON.stringify(
          {
            source: "http",
            docs_context: pack.docs_context || "",
            code_context: pack.code_context || "",
            token_usage: pack.token_usage || { total_tokens: 0, budget_remaining: 0, by_source: {} },
            provenance: (pack.provenance || []).map((e) => e.path).filter(Boolean),
            repository_state: pack.repository_state || "",
            metadata: pack.metadata || {},
          },
          null,
          2
        ),
      };
    }
    if (p.type === "OriginalPassthrough") {
      // Zero-value suppression (TASK-031): no context hits — report zeros honestly.
      return {
        output: JSON.stringify(
          {
            source: "http",
            passthrough: true,
            reason: p.reason || "",
            docs_context: "",
            code_context: "",
            token_usage: { total_tokens: 0, budget_remaining: 0, by_source: {} },
          },
          null,
          2
        ),
      };
    }
    throw new Error(`unexpected payload type: ${p.type}`);
  }
};
