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
    log("daemon not ready; skipping session context");
    return {};
  }
  const repositoryPath = input?.cwd || process.cwd();
  const probe =
    "Give a concise overview of this repository: project purpose, structure, key " +
    "modules, and conventions. Use this to seed the session context.";
  const out = await mcpCall("tools/call", {
    name: "knocode_context",
    arguments: { prompt: probe, repository_path: repositoryPath },
  });
  if (out.kind !== "ok") return {};
  const text = resultText(out.result);
  if (!text) return {};
  return {
    hookSpecificOutput: {
      hookEventName: "SessionStart",
      additionalContext: `[knocode] repository context:\n${text}`,
    },
  };
}

async function handleUserPromptSubmit(input) {
  const prompt = input?.prompt;
  if (!prompt || typeof prompt !== "string" || prompt.trim().length === 0) return {};
  const repositoryPath = input?.cwd || process.cwd();
  const out = await mcpCall("tools/call", {
    name: "knocode_context",
    arguments: { prompt, repository_path: repositoryPath },
  });
  if (out.kind !== "ok") return {};
  const text = resultText(out.result);
  if (!text) return {};
  return {
    hookSpecificOutput: {
      hookEventName: "UserPromptSubmit",
      additionalContext: `[knocode] repository context:\n${text}`,
    },
  };
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