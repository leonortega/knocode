#!/usr/bin/env node
/**
 * Knocode agent-hook handler for GitHub Copilot (VS Code).
 *
 * Reads the hook event JSON from stdin, calls the local Knocode daemon over MCP
 * (POST /mcp on http://127.0.0.1:9527), and writes a single JSON object to stdout
 * — the exact shape VS Code expects. It never prints anything else to stdout.
 *
 * Events (passed as argv[2], also read from hook_event_name):
 *   user-prompt-submit   -> enrich the user's prompt via knocode_context (additionalContext)
 *   pre-tool-use         -> retry fallback: enriches the cached prompt when submit failed
 *
 * Verified on VS Code 1.137 / Copilot Chat 0.65: `UserPromptSubmit` DOES honor
 * `hookSpecificOutput.additionalContext` (the Hooks log records it under Output
 * and the agent answers with repo context), despite the reference docs claiming
 * that event "uses the common output format only". So submit is the primary
 * injection point — it fires on every turn, including tool-less answers where
 * `PreToolUse` never runs. On submit failure (daemon down, timeout, passthrough)
 * the prompt is cached per session and `PreToolUse` injects consume-once on the
 * first tool call of the turn instead. Submit success writes no cache, so a turn
 * never pays double injection. There is no `SessionStart` hook: a synthetic-probe
 * overview fires once per session, is generic (not query-specific), and is stale
 * after the first turn.
 *
 * Tool-output compression is intentionally NOT handled here: RTK (github.com/rtk-ai/rtk)
 * owns the command-rewriting/compression layer — the knocode installer wires RTK's own
 * Copilot integration when the user opts in. Knocode stays focused on repository context.
 * (knocode's old PreToolUse hook was removed: RTK owns the Copilot PreToolUse layer for
 * command rewriting, and a second PreToolUse just duplicated daemon calls per tool.)
 *
 * Fail-open: any daemon error, timeout, indexing-in-progress (-32001), or missing tool
 * returns `{}` (no-op) and exits 0 — configured hooks never stall or break the agent.
 *
 * Env:
 *   KNOCODE_DAEMON_URL              daemon base URL            (default http://127.0.0.1:9527)
 *   KNOCODE_TIMEOUT_MS              per MCP call timeout (ms)  (default 15000)
 *   PLUGIN_DATA                     writable plugin state dir (cache location; falls back to os.tmpdir())
 *
 * Requires Node.js >= 18 (global fetch + AbortSignal.timeout).
 */

import * as readline from "node:readline";
import { randomUUID } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const DAEMON_URL = process.env.KNOCODE_DAEMON_URL || "http://127.0.0.1:9527";
const TIMEOUT_MS = num("KNOCODE_TIMEOUT_MS", 15000);

function num(env, def) {
  const n = Number(process.env[env]);
  return Number.isFinite(n) && n > 0 ? n : def;
}

/** Logs to stderr only — stdout is reserved for the hook JSON response. */
function log(...args) {
  try {
    process.stderr.write(`[knocode-hook] ${args.join(" ")}\n`);
  } catch { /* ignore */ }
}

/**
 * Verbosity gate (mirrors knocode-client's KNOCODE_LOG_LEVEL mapping:
 * error/warn → 0 quiet, info → 1 normal, debug/trace → 2 verbose).
 */
function verbosity() {
  const map = { error: 0, warn: 0, info: 1, debug: 2, trace: 2 };
  const raw = (process.env.KNOCODE_LOG_LEVEL || "info").trim().toLowerCase();
  const v = map[raw];
  return v === undefined ? 1 : v;
}

// ---------------------------------------------------------------------------
// Daemon client (MCP)
// ---------------------------------------------------------------------------

let mcpRequestId = 0;

/**
 * Send one JSON-RPC request to the daemon's POST /mcp endpoint.
 * Returned shape lets callers fail open without throwing.
 */
async function mcpCall(method, params) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(`${DAEMON_URL}/mcp`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: ++mcpRequestId, method, params }),
      signal: controller.signal,
    });
    clearTimeout(timer);
    if (res.status === 404 || res.status === 405) return { kind: "unsupported" };
    if (!res.ok) return { kind: "failure", reason: `HTTP ${res.status}` };
    const body = await res.json();
    if (body?.error) return { kind: "error", code: body.error.code, message: body.error.message };
    if (body?.result === undefined) return { kind: "failure", reason: "malformed JSON-RPC response" };
    return { kind: "ok", result: body.result };
  } catch (err) {
    clearTimeout(timer);
    return { kind: "failure", reason: String(err) };
  }
}

// ---------------------------------------------------------------------------
// Per-turn prompt cache — fallback only. Submit is the primary injection point;
// the prompt is stashed per session_id ONLY when submit-time enrichment failed,
// so PreToolUse can retry once on the first tool call of the turn.
// ---------------------------------------------------------------------------

/** Session id from any known hook-input shape (VS Code uses snake_case). */
function sessionIdOf(input) {
  const id = input?.session_id ?? input?.sessionId ?? input?.sessionID;
  return typeof id === "string" && id.trim().length > 0 ? id : undefined;
}

function cacheDir() {
  const base = process.env.PLUGIN_DATA || os.tmpdir();
  return path.join(base, "knocode-hooks");
}

function cachePath(sessionId) {
  const safe = String(sessionId).replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 128) || "default";
  return path.join(cacheDir(), `${safe}.json`);
}

function writePromptCache(sessionId, entry) {
  try {
    fs.mkdirSync(cacheDir(), { recursive: true });
    fs.writeFileSync(cachePath(sessionId), JSON.stringify(entry), "utf8");
  } catch (err) {
    log(`prompt cache write failed: ${err?.message || err}`);
  }
}

/** Read + delete (consume-once per user turn). Returns null on cache miss. */
function consumePromptCache(sessionId) {
  const file = cachePath(sessionId);
  let raw;
  try {
    raw = fs.readFileSync(file, "utf8");
  } catch {
    return null;
  }
  try {
    fs.unlinkSync(file);
  } catch { /* best-effort */ }
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Per-event handlers — each returns a hook output object or {} (no-op)
// ---------------------------------------------------------------------------

async function handleUserPromptSubmit(input) {
  const prompt = input?.prompt;
  if (!prompt || typeof prompt !== "string" || prompt.trim().length === 0) return {};
  const repositoryPath = input?.cwd || process.cwd();
  // §7 request correlation — same contract as the OpenCode plugin (request_id echoed
  // in the daemon's "MCP knocode_context built" log line and structuredContent).
  const requestId = randomUUID();
  const startedAt = Date.now();
  const out = await mcpCall("tools/call", {
    name: "knocode_context",
    arguments: { prompt, repository_path: repositoryPath, request_id: requestId },
  });
  // Correlation/metrics line at verbosity ≥ 1; failures stay visible at verbosity 0.
  if (out.kind !== "ok" || verbosity() >= 1) {
    log(`context request_id=${requestId} kind=${out.kind} latency=${Date.now() - startedAt}ms (user-prompt-submit)`);
  }
  if (out.kind === "ok") {
    const text = resultText(out.result);
    if (text) {
      // Verbosity 2: show the payload Copilot will actually receive (first 400 chars).
      if (verbosity() >= 2) {
        const total = text.length;
        const preview = text.slice(0, 400).replace(/\r?\n/g, "\\n");
        log(`payload (${total} chars): ${preview}${total > 400 ? `… (+${total - 400} chars)` : ""}`);
      }
      // Success writes NO cache — the turn already has its context, and PreToolUse
      // must not inject a second time.
      return {
        hookSpecificOutput: {
          hookEventName: "UserPromptSubmit",
          additionalContext: userPromptContext(text),
        },
      };
    }
  }
  // Submit failed (daemon down, timeout, passthrough): cache for the PreToolUse
  // retry instead of dropping the turn's context entirely.
  const sessionId = sessionIdOf(input);
  if (sessionId) {
    writePromptCache(sessionId, {
      prompt,
      cwd: repositoryPath,
      timestamp: input?.timestamp,
    });
    if (verbosity() >= 2) log(`submit enrichment failed — prompt cached for session ${sessionId} (PreToolUse retry)`);
  } else if (verbosity() >= 2) {
    log("submit enrichment failed without session_id — prompt not cached");
  }
  return {};
}

async function handlePreToolUse(input) {
  const toolName = typeof input?.tool_name === "string" ? input.tool_name : "";
  // Never enrich knocode's own tool calls — the tool result already is context.
  if (/knocode/i.test(toolName)) return {};
  const sessionId = sessionIdOf(input);
  if (!sessionId) return {};
  const cached = consumePromptCache(sessionId);
  if (!cached || typeof cached.prompt !== "string" || cached.prompt.trim().length === 0) {
    return {}; // no cached prompt (later tool call in the same turn) — inject once only
  }
  const repositoryPath = cached.cwd || input?.cwd || process.cwd();
  const prompt = cached.prompt;
  // §7 request correlation — same contract as the OpenCode plugin (request_id echoed
  // in the daemon's "MCP knocode_context built" log line and structuredContent).
  const requestId = randomUUID();
  const startedAt = Date.now();
  const out = await mcpCall("tools/call", {
    name: "knocode_context",
    arguments: { prompt, repository_path: repositoryPath, request_id: requestId },
  });
  // Correlation/metrics line at verbosity ≥ 1; failures stay visible at verbosity 0.
  if (out.kind !== "ok" || verbosity() >= 1) {
    log(`context request_id=${requestId} kind=${out.kind} latency=${Date.now() - startedAt}ms (pre-tool-use tool=${toolName || "?"})`);
  }
  if (out.kind !== "ok") return {};
  const text = resultText(out.result);
  if (!text) return {};
  // Verbosity 2: show the payload Copilot will actually receive (first 400 chars).
  if (verbosity() >= 2) {
    const total = text.length;
    const preview = text.slice(0, 400).replace(/\r?\n/g, "\\n");
    log(`payload (${total} chars): ${preview}${total > 400 ? `… (+${total - 400} chars)` : ""}`);
  }
  return {
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      additionalContext: userPromptContext(text),
    },
  };
}

// ---------------------------------------------------------------------------
// Prompt-prefix split — the daemon's knocode_context answer is a FULL replacement
// ("<original prompt>\n\n---\n\nContext:\n<yaml>"). VS Code hooks can only APPEND
// context alongside the prompt the user already submitted, so appending the whole
// replacement would put the user's own prompt inside the "additional" context and
// duplicate it against the real prompt. Strip the daemon's prefix and inject only
// the context block; the marker guard doubles as prompt-duplication prevention.
// ---------------------------------------------------------------------------

/**
 * The delimiter the daemon emits between the original prompt and the context YAML
 * (http_server.rs: format!("{}\n\n---\n\nContext:\n{}", message, yaml)).
 */
const CONTEXT_DELIMITER = "\n\n---\n\nContext:\n";

/**
 * Split a daemon replacement into { prefix, context } for the append-only hook
 * surface. `context` is null when the text doesn't carry the daemon delimiter
 * (defensive: enrichment text without the wire format is injected verbatim).
 */
function splitContextPrefix(text) {
  const idx = text.indexOf(CONTEXT_DELIMITER);
  if (idx < 0) return { prefix: text, context: null };
  return { prefix: text.slice(0, idx), context: text.slice(idx + CONTEXT_DELIMITER.length) };
}

/**
 * Build the PreToolUse additionalContext for a daemon replacement:
 * context block only (never the prompt prefix — VS Code already has it).
 */
function userPromptContext(replacement) {
  const { context } = splitContextPrefix(replacement);
  return context === null
    ? `[knocode] repository context:\n${replacement}`
    : `[knocode] repository context (for your prompt above):\n${context}`;
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

async function run(event, input) {
  try {
    switch (event) {
      case "user-prompt-submit":
      case "userpromptsubmit":
        return await handleUserPromptSubmit(input);
      case "pre-tool-use":
      case "pretooluse":
        return await handlePreToolUse(input);
      default:
        log(`unknown event: ${event}`);
        return {};
    }
  } catch (err) {
    log(`hook error: ${err?.message || err}`);
    return {};
  }
}

function main() {
  const event = (process.argv[2] || "").toLowerCase();
  // The hook input may be a single JSON line or pretty-printed across lines; buffer
  // until it parses, then respond once and exit without waiting for stdin to close.
  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  let buffer = "";
  rl.on("line", (line) => {
    buffer += line + "\n";
    if (!buffer.trim()) return;
    let input = null;
    try {
      input = JSON.parse(buffer.trim());
    } catch {
      return; // not complete yet — wait for more lines
    }
    rl.close();
    run(event, input ?? {}).then((out) => {
      // Flush stdout, then let the process exit naturally (set exitCode + close stdin).
      // Abrupt `process.exit()` right after a fetch can trigger a libuv fail-fast on
      // Windows (uv async handle closing) — natural teardown avoids it.
      process.stdout.write(JSON.stringify(out) + "\n", () => {
        process.exitCode = 0;
        process.stdin.destroy();
      });
      // Hard safety net so a stubborn stdin never hangs a hook past its budget.
      setTimeout(() => process.exit(0), 3000).unref();
    });
  });
}

main();
/** Extract natural language result text; returns null on error/passthrough/empty. */
function resultText(result) {
  if (!result || result.isError === true) return null;
  const text = result?.content?.[0]?.text;
  if (!text || typeof text !== "string") return null;
  if (result?.structuredContent?.passthrough === true) return null; // zero context hits
  return text;
}