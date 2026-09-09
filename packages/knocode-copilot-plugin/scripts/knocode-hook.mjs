#!/usr/bin/env node
/**
 * Knocode agent-hook handler for GitHub Copilot (VS Code).
 *
 * Reads the hook event JSON from stdin, calls the local Knocode daemon over MCP
 * (POST /mcp on http://127.0.0.1:9527), and writes a single JSON object to stdout
 * — the exact shape VS Code expects. It never prints anything else to stdout.
 *
 * Events (passed as argv[2], also read from hook_event_name):
 *   session-start        -> inject repository context via knocode_context  (additionalContext)
 *   user-prompt-submit   -> enrich the user's prompt context via knocode_context (additionalContext)
 *
 * Tool-output compression is intentionally NOT handled here: RTK (github.com/rtk-ai/rtk)
 * owns the command-rewriting/compression layer — the knocode installer wires RTK's own
 * Copilot integration when the user opts in. Knocode stays focused on repository context.
 *
 * The hooks that run this script can ONLY inject extra context or block — VS Code does
 * not expose a prompt-rewrite hook. So `UserPromptSubmit` is the faithful analog of the
 * opencode plugin's `session.prompt` admission hook: context is fetched from the USER'S
 * ACTUAL PROMPT (not a synthetic probe), while `SessionStart` seeds a warm overview.
 * (knocode's old PreToolUse hook was removed: RTK owns the Copilot PreToolUse layer for
 * command rewriting, and a second PreToolUse just duplicated daemon calls per tool.)
 *
 * Fail-open: any daemon error, timeout, indexing-in-progress (-32001), or missing tool
 * returns `{}` (no-op) and exits 0 — configured hooks never stall or break the agent.
 *
 * Env:
 *   KNOCODE_DAEMON_URL              daemon base URL            (default http://127.0.0.1:9527)
 *   KNOCODE_TIMEOUT_MS              per MCP call timeout (ms)  (default 15000)
 *   KNOCODE_READY_TIMEOUT_MS        session-start readiness    (default 5000, 0 disables)
 *
 * Requires Node.js >= 18 (global fetch + AbortSignal.timeout).
 */

import * as readline from "node:readline";
import { randomUUID } from "node:crypto";

const DAEMON_URL = process.env.KNOCODE_DAEMON_URL || "http://127.0.0.1:9527";
const TIMEOUT_MS = num("KNOCODE_TIMEOUT_MS", 15000);
const READY_TIMEOUT_MS = num("KNOCODE_READY_TIMEOUT_MS", 5000);
const READY_POLL_MS = 250;

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

/** Wait (bounded, fail-open) until the daemon reports ready via GET /health. */
async function daemonReady(timeoutMs) {
  if (!timeoutMs) return true; // readiness disabled
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const remaining = deadline - Date.now();
    const controller = new AbortController();
    // Manual timeout (NOT AbortSignal.timeout): its native timer can crash libuv
    // on Windows when combined with process.exit() right after a response.
    const timer = setTimeout(() => controller.abort(), Math.min(2000, Math.max(1, remaining)));
    try {
      const res = await fetch(`${DAEMON_URL}/health`, { signal: controller.signal });
      clearTimeout(timer);
      if (res.ok) {
        let state;
        try { state = (await res.json())?.state; } catch { /* non-JSON body => ready */ }
        if (state === undefined || state === "ready") return true;
      }
    } catch {
      clearTimeout(timer);
      return false; // unreachable
    }
    await new Promise((r) => setTimeout(r, READY_POLL_MS));
  }
  return false;
}

// ---------------------------------------------------------------------------
// Per-event handlers — each returns a hook output object or {} (no-op)
// ---------------------------------------------------------------------------

async function handleSessionStart(input) {
  if (READY_TIMEOUT_MS && !(await daemonReady(READY_TIMEOUT_MS))) {
    if (verbosity() >= 1) log("daemon not ready; skipping session context");
    return {};
  }
  const repositoryPath = input?.cwd || process.cwd();
  const probe =
    "Give a concise overview of this repository: project purpose, structure, key " +
    "modules, and conventions. Use this to seed the session context.";
  // §7 request correlation: client-generated id echoed by the daemon in its log line
  // and structuredContent, so hook-side and daemon-side lines join per request.
  const requestId = randomUUID();
  const startedAt = Date.now();
  const out = await mcpCall("tools/call", {
    name: "knocode_context",
    arguments: { prompt: probe, repository_path: repositoryPath, request_id: requestId },
  });
  // Correlation/metrics line at verbosity ≥ 1; failures stay visible at verbosity 0.
  if (out.kind !== "ok" || verbosity() >= 1) {
    log(`context request_id=${requestId} kind=${out.kind} latency=${Date.now() - startedAt}ms (session-start)`);
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
      hookEventName: "SessionStart",
      // Context block only — the replacement's prefix is the synthetic probe text,
      // which is noise in a session-seed digest.
      additionalContext: `[knocode] repository context:\n${(splitContextPrefix(text).context ?? text)}`,
    },
  };
}

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
      hookEventName: "UserPromptSubmit",
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
 * Build the UserPromptSubmit additionalContext for a daemon replacement:
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
      case "session-start":
      case "sessionstart":
        return await handleSessionStart(input);
      case "user-prompt-submit":
      case "userpromptsubmit":
        return await handleUserPromptSubmit(input);
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