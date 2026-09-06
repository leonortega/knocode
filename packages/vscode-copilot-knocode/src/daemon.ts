/**
 * Knocode daemon client for the `@knocode` Copilot chat participant.
 *
 * Reuses the exact wire contract of `packages/opencode-knocode`: MCP over
 * `POST /mcp` (`knocode_context`; compression lives in RTK) with a `GET /health`
 * readiness gate. Fail-open: any error, timeout, indexing-in-progress, or
 * zero-hit returns `null` so the participant always runs with the bare prompt.
 */

const DEFAULT_DAEMON_URL = "http://127.0.0.1:9527";
const DEFAULT_TIMEOUT_MS = 30_000;
const READY_TIMEOUT_MS = 5_000;
const READY_POLL_MS = 250;
const READY_REPOLL_MS = 30_000;

export function getDaemonUrl(): string {
  return process.env.KNOCODE_DAEMON_URL || DEFAULT_DAEMON_URL;
}

export function getTimeoutMs(): number {
  const raw = process.env.KNOCODE_TIMEOUT_MS;
  if (raw) {
    const n = Number(raw);
    if (Number.isFinite(n) && n > 0) return n;
  }
  return DEFAULT_TIMEOUT_MS;
}

export type McpCallOutcome =
  | { kind: "ok"; result: any }
  | { kind: "error"; code: number; message: string }
  | { kind: "unsupported"; status: number }
  | { kind: "failure"; reason: string };

/**
 * Minimal response shape the daemon client needs. We cast the native fetch
 * result to this because some @types/node versions resolve the merged global
 * `Response` to `{}` (their fetch.d.ts conditional collapses without DOM).
 */
export interface KnocodeFetchResponse {
  ok: boolean;
  status: number;
  json(): Promise<any>;
}

let mcpRequestId = 0;

/**
 * Send one JSON-RPC request to the daemon's `POST /mcp` endpoint.
 */
export async function mcpCall(
  method: string,
  params: any,
  opts?: { url?: string; timeoutMs?: number; fetchImpl?: (input: any, init?: any) => Promise<KnocodeFetchResponse> },
): Promise<McpCallOutcome> {
  const url = opts?.url ?? getDaemonUrl();
  const timeoutMs = opts?.timeoutMs ?? getTimeoutMs();
  const fetchFn = opts?.fetchImpl ?? (fetch as any);
  const controller = new AbortController();
  // Manual timeout (not AbortSignal.timeout): the native timer can crash libuv
  // on Windows when combined with immediate process teardown.
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res: KnocodeFetchResponse = await fetchFn(`${url}/mcp`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: ++mcpRequestId, method, params }),
      signal: controller.signal,
    });
    clearTimeout(timer);
    if (res.status === 404 || res.status === 405) return { kind: "unsupported", status: res.status };
    if (!res.ok) return { kind: "failure", reason: `HTTP ${res.status}` };
    const body: any = await res.json();
    if (body?.error) return { kind: "error", code: body.error.code, message: body.error.message };
    if (body?.result === undefined) return { kind: "failure", reason: "malformed JSON-RPC response" };
    return { kind: "ok", result: body.result };
  } catch (err) {
    clearTimeout(timer);
    return { kind: "failure", reason: String(err) };
  }
}

/**
 * Wait (bounded, fail-open) until the daemon reports ready via `GET /health`.
 * Returns false when unreachable or when the budget expires while indexing.
 */
export async function waitForDaemonReady(
  opts?: { url?: string; timeoutMs?: number; fetchImpl?: (input: any, init?: any) => Promise<KnocodeFetchResponse> },
): Promise<boolean> {
  const url = opts?.url ?? getDaemonUrl();
  const timeoutMs = opts?.timeoutMs ?? READY_TIMEOUT_MS;
  const fetchFn = opts?.fetchImpl ?? (fetch as any);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const remaining = deadline - Date.now();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), Math.min(2000, Math.max(1, remaining)));
    try {
      const res: KnocodeFetchResponse = await fetchFn(`${url}/health`, { signal: controller.signal });
      clearTimeout(timer);
      if (res.ok) {
        let state: string | undefined;
        try {
          const body = (await res.json()) as { state?: string };
          state = body?.state;
        } catch {
          /* 200 with non-JSON body — daemon is up, treat as ready */
        }
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

// Cache readiness so a possibly-slow wait never gates every turn.
let readinessCheckedAt = 0;

export async function ensureDaemonReady(): Promise<void> {
  const now = Date.now();
  if (now - readinessCheckedAt < READY_REPOLL_MS) return;
  readinessCheckedAt = now;
  await waitForDaemonReady();
}

/**
 * Pre-model context enrichment: drive `knocode_context` via MCP. Returns the
 * enriched context text to prepend, or `null` for a bare-prompt passthrough.
 */
export async function requestContextEnrichment(
  message: string,
  repositoryPath: string | undefined,
  opts?: { url?: string; timeoutMs?: number; fetchImpl?: (input: any, init?: any) => Promise<KnocodeFetchResponse> },
): Promise<string | null> {
  const out = await mcpCall(
    "tools/call",
    {
      name: "knocode_context",
      arguments: {
        prompt: message,
        ...(repositoryPath ? { repository_path: repositoryPath } : {}),
      },
    },
    opts,
  );
  if (out.kind !== "ok") return null;
  const text: string | undefined = out.result?.content?.[0]?.text;
  const passthrough = out.result?.structuredContent?.passthrough === true;
  const isError = out.result?.isError === true;
  if (!text || passthrough || isError) return null;
  return text;
}