#!/usr/bin/env node
/**
 * Self-contained smoke test for scripts/knocode-hook.mjs.
 * Starts a mock daemon (POST /mcp), spawns the hook handler against it, and
 * asserts the emitted hook output shape. Exits non-zero on any failure.
 * Not part of the shipped plugin — test/dev only.
 *
 * Flow under test (verified on VS Code 1.137: UserPromptSubmit DOES honor
 * additionalContext — the primary injection point, including tool-less turns):
 *   1. user-prompt-submit (daemon up)   -> additionalContext, no cache write
 *   2. pre-tool-use after success       -> {} (no double injection)
 *   3. user-prompt-submit (daemon down) -> {} + cached for retry
 *   4. pre-tool-use (daemon up)         -> additionalContext (retry, consume-once)
 *   5. pre-tool-use again               -> {} (consume-once, no second call)
 *   6. knocode tool call                -> {} (never enrich knocode's own tool)
 */
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const hook = resolve(dirname(fileURLToPath(import.meta.url)), "../scripts/knocode-hook.mjs");
const PORT = 9529;
const PLUGIN_DATA = mkdtempSync(join(tmpdir(), "knocode-smoke-"));

function runHook(event, input, daemonUrl) {
  return new Promise((ok) => {
    const child = spawn(process.execPath, [hook, event], {
      env: { ...process.env, KNOCODE_DAEMON_URL: daemonUrl ?? `http://127.0.0.1:${PORT}`, KNOCODE_LOG_LEVEL: "debug", PLUGIN_DATA },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let out = "", err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    const t = setTimeout(() => child.kill(), 5000);
    child.on("close", (code) => {
      clearTimeout(t);
      ok({ code, out: out.trim(), err: err.trim() });
    });
    child.stdin.end(JSON.stringify(input));
  });
}

const seen = { requestIds: [] };

const server = createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    res.writeHead(200, { "Content-Type": "application/json" });
    if (req.url === "/health") return res.end(JSON.stringify({ state: "ready" }));
    const r = JSON.parse(body);
    const args = r?.params?.arguments || {};
    // §7 request correlation contract: every knocode_context call carries a request_id.
    if (typeof args.request_id === "string" && args.request_id.length > 0) {
      seen.requestIds.push(args.request_id);
    }
    // Real daemon wire format: the answer is a FULL replacement —
    // "<prompt>\n\n---\n\nContext:\n<yaml>" (http_server.rs).
    const text = `${args.prompt || "?"}\n\n---\n\nContext:\ncode_context: for ${args.prompt?.slice(0, 40) || "?"}`;
    res.end(JSON.stringify({ jsonrpc: "2.0", id: r.id, result: { content: [{ type: "text", text }], structuredContent: { type: "context", passthrough: false, request_id: args.request_id ?? null }, isError: false } }));
  });
});

function parseOut(r, name) {
  try {
    return { obj: JSON.parse(r.out) };
  } catch {
    console.error(`FAIL ${name}: non-JSON out: ${r.out}`);
    return { failed: true };
  }
}

server.listen(PORT, async () => {
  let failed = false;
  const DOWN_URL = "http://127.0.0.1:1"; // nothing listens here — daemon-down path

  function checkContext(r, name, eventName, sentinel) {
    const { obj, failed: pf } = parseOut(r, name);
    if (pf) return null;
    if (r.code !== 0) { console.error(`FAIL ${name}: exit ${r.code}: ${r.err}`); return null; }
    const ctx = obj?.hookSpecificOutput?.additionalContext;
    if (obj?.hookSpecificOutput?.hookEventName !== eventName) { console.error(`FAIL ${name}: wrong hookEventName: ${r.out.slice(0, 120)}`); return null; }
    if (!ctx) { console.error(`FAIL ${name}: missing additionalContext: ${r.out}`); return null; }
    // The daemon's replacement embeds the prompt as its prefix; additionalContext
    // must carry the context block only, or the user's prompt reaches the model twice.
    if (sentinel && ctx.includes(sentinel)) { console.error(`FAIL ${name}: additionalContext duplicates the prompt prefix: ${ctx.slice(0, 120)}`); return null; }
    if (!ctx.includes("code_context:")) { console.error(`FAIL ${name}: additionalContext missing the context block: ${ctx.slice(0, 120)}`); return null; }
    if (!r.err.includes("payload (")) { console.error(`FAIL ${name}: no payload preview at verbosity 2 (stderr: ${r.err.slice(0, 200)})`); return null; }
    console.log(`PASS ${name}: exit=0, ctx=${JSON.stringify(ctx.slice(0, 40))}, no prompt duplication, payload preview logged`);
    return ctx;
  }

  function checkEmpty(r, name, extra) {
    const { obj, failed: pf } = parseOut(r, name);
    if (pf) return false;
    if (r.code !== 0) { console.error(`FAIL ${name}: exit ${r.code}: ${r.err}`); return false; }
    if (obj?.hookSpecificOutput !== undefined) { console.error(`FAIL ${name}: must return {}${extra || ""}: ${r.out}`); return false; }
    console.log(`PASS ${name}: exit=0, returned {}`);
    return true;
  }

  // 1. user-prompt-submit injects directly (primary path, covers tool-less turns).
  const sid = "s-smoke-1";
  const r1 = await runHook("user-prompt-submit", { prompt: "Where is the checkout flow implemented? USER-PROMPT-SENTINEL", cwd: "C:/repo", session_id: sid, hook_event_name: "UserPromptSubmit" });
  if (!checkContext(r1, "user-prompt-submit-inject", "UserPromptSubmit", "USER-PROMPT-SENTINEL")) failed = true;

  // 2. pre-tool-use after a successful submit injects nothing (no double pay).
  const r2 = await runHook("pre-tool-use", { tool_name: "editFiles", tool_input: { files: ["src/main.ts"] }, session_id: sid, cwd: "C:/repo", hook_event_name: "PreToolUse" });
  if (!checkEmpty(r2, "pre-tool-use-after-success", " (submit already injected)")) failed = true;

  // 3. user-prompt-submit with daemon down returns {} but caches for retry.
  const sid2 = "s-smoke-2";
  const callsBeforeDown = seen.requestIds.length;
  const r3 = await runHook("user-prompt-submit", { prompt: "Retry this prompt after the daemon came back online FALLBACK-SENTINEL", cwd: "C:/repo", session_id: sid2, hook_event_name: "UserPromptSubmit" }, DOWN_URL);
  if (!checkEmpty(r3, "user-prompt-submit-daemon-down", " (fail-open, cached for retry)")) failed = true;
  if (seen.requestIds.length !== callsBeforeDown) { console.error(`FAIL daemon-down: mock daemon must not see the failed call`); failed = true; }

  // 4. pre-tool-use retries the cached prompt (consume-once).
  const r4 = await runHook("pre-tool-use", { tool_name: "editFiles", tool_input: { files: ["src/main.ts"] }, session_id: sid2, cwd: "C:/repo", hook_event_name: "PreToolUse" });
  if (!checkContext(r4, "pre-tool-use-retry", "PreToolUse", "FALLBACK-SENTINEL")) failed = true;

  // 5. Second pre-tool-use in the same turn is consume-once -> {}.
  const r5 = await runHook("pre-tool-use", { tool_name: "editFiles", tool_input: { files: ["src/other.ts"] }, session_id: sid2, cwd: "C:/repo", hook_event_name: "PreToolUse" });
  if (!checkEmpty(r5, "pre-tool-use-consume-once", " (second tool call)")) failed = true;

  // 6. knocode's own tool calls are never enriched.
  const sid3 = "s-smoke-3";
  await runHook("user-prompt-submit", { prompt: "lookup SKIP-SENTINEL", cwd: "C:/repo", session_id: sid3, hook_event_name: "UserPromptSubmit" }, DOWN_URL);
  const callsBefore = seen.requestIds.length;
  const r6 = await runHook("pre-tool-use", { tool_name: "knocode_context", tool_input: {}, session_id: sid3, cwd: "C:/repo", hook_event_name: "PreToolUse" });
  if (!checkEmpty(r6, "pre-tool-use-skip-knocode-tool", " (knocode tool)")) failed = true;
  else if (seen.requestIds.length !== callsBefore) { console.error(`FAIL pre-tool-use-skip-knocode-tool: knocode tool must not call daemon`); failed = true; }

  if (seen.requestIds.length !== 2) {
    console.error(`FAIL request_id: expected 2 correlated calls (submit inject + pre-tool-use retry), got ${seen.requestIds.length}`);
    failed = true;
  } else {
    console.log(`PASS request_id: ${seen.requestIds.map((id) => id.slice(0, 8)).join(", ")} (echoed in structuredContent)`);
  }

  server.close();
  try { rmSync(PLUGIN_DATA, { recursive: true, force: true }); } catch { /* ignore */ }
  process.exit(failed ? 1 : 0);
});
