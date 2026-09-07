/**
 * Knocode daemon client — SINGLE SOURCE OF TRUTH.
 *
 * This module is vendored (source-copied at build time) into the agent plugins:
 *   - packages/opencode-knocode        (ESM — NodeNext)
 *   - packages/vscode-copilot-knocode  (CJS  — packaged .vsix)
 * via each package's scripts/vendor-client.mjs. Edit behavior HERE, not in the
 * vendored copies; CI and the packages' prebuild/pretest hooks fail if a copy
 * drifts from this file.
 *
 * Wire contract (v0.9.x):
 *   - MCP over `POST /mcp` (JSON-RPC 2.0): `initialize`, `notifications/initialized`,
 *     `ping`, `tools/list`, `tools/call knocode_context`.
 *   - Readiness gate over `GET /health` (`state: "ready" | "indexing"`).
 *   - Tool-output compression is NOT handled here: RTK (github.com/rtk-ai/rtk) owns
 *     that layer.
 *   - Every `knocode_context` call carries a client-generated `request_id` (§7 request
 *     correlation): the daemon echoes it in its log lines and `structuredContent`, so
 *     a plugin log line joins with the daemon log line for the same request.
 *   - Fail-open: any error, timeout, indexing-in-progress, or zero-hit returns a
 *     tagged `ContextPassthrough` so the caller always runs with the bare prompt.
 *
 * Host shims are injectable: `fetchImpl` for tests and alternative runtimes, and
 * `timeoutFactory` for hosts where `AbortSignal.timeout` is unsafe (the VS Code
 * extension uses a manual controller because that signal can crash libuv on Windows
 * during immediate process teardown).
 */

import { randomUUID } from "node:crypto";

// ── Config ───────────────────────────────────────────────────────────────────

export const DEFAULT_DAEMON_URL = "http://127.0.0.1:9527";
export const DEFAULT_TIMEOUT_MS = 30_000;
/** How long the first request waits for the daemon to finish indexing before fail-open. */
export const DEFAULT_READY_TIMEOUT_MS = 10_000;
export const DEFAULT_READY_POLL_MS = 250;

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

export function getReadyTimeoutMs(): number {
  const raw = process.env.KNOCODE_READY_TIMEOUT_MS;
  if (raw) {
    const n = Number(raw);
    if (Number.isFinite(n) && n > 0) return n;
  }
  return DEFAULT_READY_TIMEOUT_MS;
}

/**
 * The minimal fetch response shape the client needs. Hosts cast their native
 * fetch result to this: some @types/node versions resolve the merged global
 * `Response` to `{}` (their fetch.d.ts conditional collapses without DOM).
 */
export interface KnocodeFetchResponse {
  ok: boolean;
  status: number;
  json(): Promise<any>;
}

/** Default per-call timeout: `AbortSignal.timeout`. */
function defaultTimeoutFactory(ms: number): AbortSignal {
  return AbortSignal.timeout(ms);
}

export type TimeoutFactory = (ms: number) => AbortSignal;

export interface ClientOptions {
  url?: string;
  timeoutMs?: number;
  fetchImpl?: (input: any, init?: any) => Promise<KnocodeFetchResponse>;
  /** Host-specific timeout signal strategy (see module doc). */
  timeoutFactory?: TimeoutFactory;
}

// ── MCP client — JSON-RPC 2.0 over POST /mcp ────────────────────────────────
// The daemon hosts an MCP surface on its HTTP listener (`POST /mcp`) so plugins can
// drive Knocode with typed tools (`tools/call`). This client is stateless (the
// daemon's MCP subset needs no session) and fail-open: any failure → passthrough.

export type McpCallOutcome =
  | { kind: "ok"; result: any }
  | { kind: "error"; code: number; message: string }
  // The daemon does not expose /mcp (HTTP 404/405).
  | { kind: "unsupported"; status: number }
  | { kind: "failure"; reason: string };

let mcpRequestId = 0;

/**
 * Send one JSON-RPC request to the daemon's `POST /mcp` endpoint.
 */
export async function mcpCall(
  method: string,
  params: any,
  opts?: ClientOptions,
): Promise<McpCallOutcome> {
  const url = opts?.url ?? getDaemonUrl();
  const timeoutMs = opts?.timeoutMs ?? getTimeoutMs();
  const fetchFn = opts?.fetchImpl ?? (fetch as any);
  const newTimeoutSignal = opts?.timeoutFactory ?? defaultTimeoutFactory;
  const id = ++mcpRequestId;

  try {
    const res: KnocodeFetchResponse = await fetchFn(`${url}/mcp`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
      signal: newTimeoutSignal(timeoutMs),
    });

    if (res.status === 404 || res.status === 405) {
      return { kind: "unsupported", status: res.status };
    }
    if (!res.ok) {
      return { kind: "failure", reason: `HTTP ${res.status}` };
    }

    let body: any;
    try {
      body = await res.json();
    } catch {
      return { kind: "failure", reason: "non-JSON /mcp response" };
    }
    // JSON-RPC application error (e.g. -32001 daemon_indexing) — NOT "unsupported":
    // the daemon speaks MCP, it just can't serve this call right now.
    if (body?.error) {
      return { kind: "error", code: body.error.code, message: body.error.message };
    }
    if (body?.result === undefined) {
      return { kind: "failure", reason: "malformed JSON-RPC response" };
    }
    return { kind: "ok", result: body.result };
  } catch (error) {
    console.error(`[knocode] Daemon unreachable: ${error}`);
    return { kind: "failure", reason: String(error) };
  }
}

// ── Readiness gate ───────────────────────────────────────────────────────────

/**
 * Wait until the daemon reports ready via `GET /health` (parity with the UDS Probe).
 *
 * The HTTP health/metrics listener binds BEFORE the initial index, so during a cold
 * start `/health` answers `{"state": "indexing"}` and enrichment calls fail — polling
 * here means the first real request gets context instead of an instant passthrough.
 *
 * Returns:
 *  - `true`  once `/health` reports `state: "ready"` (a 200 without a parseable
 *            state is treated as ready — a live daemon is better than a strict one)
 *  - `false` when the daemon is UNREACHABLE (connection refused — not running),
 *            so a missing daemon never stalls a hook for the full budget
 *  - `false` when the budget (`timeoutMs`) expires while the daemon keeps indexing
 *
 * Fail-open: callers proceed with the POST regardless and rely on passthrough.
 */
export async function waitForDaemonReady(
  opts: ClientOptions & { pollMs?: number } = {},
): Promise<boolean> {
  const url = opts.url ?? getDaemonUrl();
  const timeoutMs = opts.timeoutMs ?? getReadyTimeoutMs();
  const pollMs = opts.pollMs ?? DEFAULT_READY_POLL_MS;
  const fetchFn = opts.fetchImpl ?? (fetch as any);
  const newTimeoutSignal = opts.timeoutFactory ?? defaultTimeoutFactory;
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    const remaining = deadline - Date.now();
    try {
      const res: KnocodeFetchResponse = await fetchFn(`${url}/health`, {
        // Per-poll cap (≤2s): a local daemon answers in ms; a stall means it is not healthy.
        signal: newTimeoutSignal(Math.min(2_000, Math.max(1, remaining))),
      });
      if (res.ok) {
        let state: string | undefined;
        try {
          const body: any = await res.json();
          state = body?.state;
        } catch {
          // 200 with a non-JSON body — daemon is up; treat as ready.
        }
        if (state === undefined || state === "ready") return true;
        // Reachable but still indexing — keep polling until the deadline.
      }
      // Reachable with an error status — keep polling until the deadline.
    } catch {
      // Unreachable (connection refused / aborted) — the daemon is not running.
      return false;
    }
    await new Promise((r) => setTimeout(r, pollMs));
  }
  return false;
}

// Cache readiness so a possibly-slow wait never gates every request. Results are
// cached unconditionally: once the daemon is actually ready a skipped gate doesn't
// matter (the POST succeeds on its own), and a mid-index daemon only costs the wait
// budget once per window.
const READY_REPOLL_MS = 30_000;
let readinessCheckedAt = 0;

/** Bounded readiness wait, re-polled at most once per `READY_REPOLL_MS` window. */
export async function ensureDaemonReady(opts: ClientOptions & { pollMs?: number } = {}): Promise<void> {
  const now = Date.now();
  if (now - readinessCheckedAt < READY_REPOLL_MS) return;
  const waitStartedAt = Date.now();
  const ready = await waitForDaemonReady(opts);
  readinessCheckedAt = Date.now();
  if (ready && Date.now() - waitStartedAt > 1_000) {
    console.log(`[knocode] Daemon became ready after ${Date.now() - waitStartedAt}ms`);
  }
}

/** Test-only: reset the readiness cooldown so `ensureDaemonReady` waits again. */
export function resetReadinessCache(): void {
  readinessCheckedAt = 0;
}

// ── Context outcomes ─────────────────────────────────────────────────────────

/**
 * Client-generated correlation id (§7 request correlation): sent to the daemon as
 * `request_id`, echoed back in `structuredContent`, and stamped on the host plugin's
 * integration-boundary log lines so a plugin log line can be joined with the daemon's
 * log line for the same request — cheap traceability without distributed tracing.
 */
export function newRequestId(): string {
  return randomUUID();
}

/**
 * Successful enrichment outcome, with the metadata needed for the host plugin's
 * integration-boundary metrics line (latency is measured by the caller).
 */
export type ContextEnrichment = {
  kind: "enriched";
  /** Full replacement text for the user prompt (original text preserved as prefix). */
  enrichedText: string;
  /** Reported token size of the injected context pack (daemon `total_tokens`). */
  tokens: number;
  /** Number of file provenance entries in the context pack. */
  files: number;
  /** Correlation id sent to the daemon (`request_id`). */
  requestId: string;
};

/**
 * No enrichment applied — the caller must leave the user's prompt byte-identical.
 * Carries the daemon's passthrough `reason` when the daemon reported one, so the
 * plugin log distinguishes `no_context_hits` (normal) from `daemon_indexing`,
 * `daemon_unreachable`, and transport failures without reading daemon logs.
 */
export type ContextPassthrough = {
  kind: "passthrough";
  /** Why nothing was injected (daemon reason, or a client-side classification). */
  reason: string;
  /** Correlation id sent to the daemon (`request_id`). */
  requestId: string;
};

export type ContextOutcome = ContextEnrichment | ContextPassthrough;

/**
 * Pre-model context enrichment: drive `knocode_context` via MCP. Returns the enriched
 * text (+ pack metadata) to substitute for the user prompt, or a tagged passthrough
 * outcome (no context hits, indexing in progress, daemon down, or a daemon without
 * /mcp) — the caller must leave the prompt untouched in that case.
 *
 * A `request_id` correlation id is generated per call (or taken from
 * `opts.requestId`) and sent to the daemon, which echoes it in logs and
 * `structuredContent`.
 */
export async function requestContextEnrichment(
  message: string,
  repositoryPath: string | undefined,
  opts?: ClientOptions & { requestId?: string },
): Promise<ContextOutcome> {
  const requestId = opts?.requestId ?? newRequestId();
  const out = await mcpCall(
    "tools/call",
    {
      name: "knocode_context",
      arguments: {
        prompt: message,
        ...(repositoryPath ? { repository_path: repositoryPath } : {}),
        request_id: requestId,
      },
    },
    opts,
  );

  if (out.kind !== "ok") {
    // error (e.g. -32001 while indexing), unsupported (no /mcp), or failure —
    // untouched passthrough. Classify for the log line; the raw transport error
    // was already console.error'd inside mcpCall.
    const reason =
      out.kind === "error"
        ? `mcp_error_${out.code}`
        : out.kind === "unsupported"
          ? "no_mcp_surface"
          : "daemon_unreachable";
    return { kind: "passthrough", reason, requestId };
  }

  const text: string | undefined = out.result?.content?.[0]?.text;
  const structured = out.result?.structuredContent ?? {};
  const passthrough = structured?.passthrough === true;
  const isError = out.result?.isError === true;
  if (!text || passthrough || isError) {
    // Daemon reason first (always present on 0.9.x passthroughs); then classify.
    const reason =
      structured?.reason != null
        ? String(structured.reason)
        : passthrough
          ? "unspecified"
          : isError
            ? "tool_error"
            : "malformed_response";
    return { kind: "passthrough", reason, requestId };
  }
  return {
    kind: "enriched",
    enrichedText: text,
    tokens: typeof structured?.total_tokens === "number" ? structured.total_tokens : 0,
    files: Array.isArray(structured?.provenance) ? structured.provenance.length : 0,
    requestId,
  };
}
