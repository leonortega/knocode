#!/usr/bin/env node
/**
 * Self-contained smoke test for scripts/knocode-hook.mjs.
 * Starts a mock daemon (POST /mcp), spawns the hook handler against it, and
 * asserts the emitted hook output shape. Exits non-zero on any failure.
 * Not part of the shipped plugin — test/dev only.
 */
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const hook = resolve(dirname(fileURLToPath(import.meta.url)), "../scripts/knocode-hook.mjs");
const PORT = 9529;

function runHook(event, input, done) {
  const child = spawn(process.execPath, [hook, event], {
    env: { ...process.env, KNOCODE_DAEMON_URL: `http://127.0.0.1:${PORT}`, KNOCODE_READY_TIMEOUT_MS: "2000", KNOCODE_LOG_LEVEL: "debug" },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let out = "", err = "";
  child.stdout.on("data", (d) => (out += d));
  child.stderr.on("data", (d) => (err += d));
  const t = setTimeout(() => child.kill(), 5000);
  child.on("close", (code) => {
    clearTimeout(t);
    done({ code, out: out.trim(), err: err.trim() });
  });
  child.stdin.end(JSON.stringify(input));
}

const seen = { requestIds: [] };

const server = createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    res.writeHead(200, { "Content-Type": "application/json" });
    if (req.url === "/health") return res.end(JSON.stringify({ state: "ready" }));
    const r = JSON.parse(body);
    const name = r?.params?.name;
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

server.listen(PORT, async () => {
  const results = [];
  results.push(await new Promise((ok) =>
    runHook("session-start", { cwd: "C:/repo", session_id: "s", hook_event_name: "SessionStart", source: "new" }, ok)));
  results.push(await new Promise((ok) =>
    runHook("user-prompt-submit", { prompt: "Where is the checkout flow implemented? USER-PROMPT-SENTINEL", cwd: "C:/repo" }, ok)));
  server.close();

  let failed = false;
  results.forEach((r, i) => {
    const name = ["session-start", "user-prompt-submit"][i];
    if (r.code !== 0) { console.error(`FAIL ${name}: exit ${r.code}: ${r.err}`); failed = true; return; }
    let obj;
    try { obj = JSON.parse(r.out); } catch { console.error(`FAIL ${name}: non-JSON out: ${r.out}`); failed = true; return; }
    const ctx = obj?.hookSpecificOutput?.additionalContext;
    if (!ctx) { console.error(`FAIL ${name}: missing additionalContext: ${r.out}`); failed = true; return; }
    // The daemon's replacement embeds the prompt as its prefix; additionalContext
    // must carry the context block only, or the user's prompt reaches the model twice.
    if (ctx.includes("USER-PROMPT-SENTINEL")) { console.error(`FAIL ${name}: additionalContext duplicates the prompt prefix: ${ctx.slice(0, 120)}`); failed = true; return; }
    if (!ctx.includes("code_context:")) { console.error(`FAIL ${name}: additionalContext missing the context block: ${ctx.slice(0, 120)}`); failed = true; return; }
    if (!r.err.includes("payload (")) { console.error(`FAIL ${name}: no payload preview at verbosity 2 (stderr: ${r.err.slice(0, 200)})`); failed = true; return; }
    console.log(`PASS ${name}: exit=0, ctx=${JSON.stringify(ctx.slice(0, 40))}, no prompt duplication, payload preview logged`);
  });
  if (seen.requestIds.length !== results.length) {
    console.error(`FAIL request_id: expected ${results.length} correlated calls, got ${seen.requestIds.length}`);
    failed = true;
  } else {
    console.log(`PASS request_id: ${seen.requestIds.map((id) => id.slice(0, 8)).join(", ")} (echoed in structuredContent)`);
  }
  process.exit(failed ? 1 : 0);
});