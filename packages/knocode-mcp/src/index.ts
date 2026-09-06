#!/usr/bin/env node
/**
 * Knocode MCP server — stdio → HTTP bridge to the daemon-hosted MCP surface.
 *
 * The daemon (127.0.0.1:9527) owns the canonical MCP tools (`knocode_context` —
 * repository context enrichment; tool-output compression lives in RTK,
 * github.com/rtk-ai/rtk). This process is a thin, pass-through proxy so agents that
 * require a stdio MCP server (Codex, VS Code Copilot, Claude) reach exactly the
 * same tools the opencode plugin drives — there is no local tool registry, so the
 * surface can never drift from the daemon's.
 *
 *   Codex / Copilot / Claude --stdio JSON-RPC--> knocode-mcp --> POST /mcp (daemon)
 *
 * Requests are forwarded verbatim and responses relayed back. When the daemon is
 * unreachable, `tools/call` answers a JSON-RPC -32000 error so clients can fail
 * open; MCP notifications (`notifications/initialized`) are forwarded but never
 * answered, per the stdio convention.
 */

import * as readline from "node:readline";

const DAEMON_URL = process.env.KNOCODE_DAEMON_URL || "http://127.0.0.1:9527";
const MCP_ENDPOINT = `${DAEMON_URL}/mcp`;
const TIMEOUT_MS = Number(process.env.KNOCODE_TIMEOUT_MS || "30000");

async function forward(req: unknown): Promise<{ ok: boolean; status: number; body: string } | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(MCP_ENDPOINT, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
      signal: controller.signal,
    });
    clearTimeout(timer);
    const body = await res.text();
    return { ok: res.ok, status: res.status, body };
  } catch {
    clearTimeout(timer);
    return null; // unreachable / aborted / network error
  }
}

function errorEnvelope(id: unknown, code: number, message: string): string {
  return JSON.stringify({ jsonrpc: "2.0", id: id ?? null, error: { code, message } });
}

function isNotification(req: any): boolean {
  const method = typeof req?.method === "string" ? req.method : "";
  return req?.id === undefined || req?.id === null || method.startsWith("notifications/");
}

async function handleLine(req: unknown): Promise<void> {
  const notif = isNotification(req);

  const resp = await forward(req);
  if (!resp) {
    // Daemon down — clean failure, never a silent drop for tools/call.
    if (!notif) {
      process.stdout.write(
        errorEnvelope(
          (req as any)?.id,
          -32000,
          `knocode daemon unreachable at ${DAEMON_URL} — start it with \`knocode serve\``,
        ) + "\n",
      );
    }
    return;
  }
  if (!resp.ok) {
    // Transport-level rejection (daemon predates /mcp, malformed request, ...).
    if (notif) return;
    let message = `knocode daemon /mcp returned HTTP ${resp.status}`;
    if (resp.body) {
      try {
        const parsed = JSON.parse(resp.body) as { error?: { message?: string } };
        if (parsed?.error?.message) message = parsed.error.message;
      } catch {
        /* non-JSON error body — keep the generic message */
      }
    }
    process.stdout.write(errorEnvelope((req as any)?.id, -32002, message) + "\n");
    return;
  }
  if (notif) return; // forwarded, but never answered
  const body = resp.body.trim();
  if (body) process.stdout.write(body + "\n");
}

function main(): void {
  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });

  rl.on("line", (line) => {
    if (!line.trim()) return;
    let req: unknown;
    try {
      req = JSON.parse(line);
    } catch {
      return; // unparseable line — ignore rather than desync the JSON-RPC stream
    }
    handleLine(req).catch(() => {
      if (!isNotification(req)) {
        process.stdout.write(errorEnvelope((req as any)?.id, -32603, "internal knocode-mcp error") + "\n");
      }
    });
  });
}

main();
