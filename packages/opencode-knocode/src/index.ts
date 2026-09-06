/**
 * Knocode AI Runtime Plugin for OpenCode (V2 plugin spec)
 *
 * OpenCode V2 plugin API (@opencode-ai/plugin "beta", Plugin.define + ctx.session.hook):
 * the `session.prompt` hook intercepts the incoming user prompt BEFORE attachment and
 * skill resolution and durable inbox admission, and receives an owned, mutable draft
 * (`prompt.text`, `prompt.files`, `metadata`, `delivery`). Edits become the canonical
 * persisted user input for that admission.
 *
 * Knocode uses it to enrich the prompt with repository context from the daemon
 * (MCP `POST /mcp` → `tools/call knocode_context`).
 *
 * Tool-output compression is intentionally NOT handled here: RTK (github.com/rtk-ai/rtk)
 * owns the command-rewriting/compression layer — the knocode installer wires RTK's own
 * integrations when the user opts in. Knocode stays focused on repository context.
 *
 * Fail-open: any daemon error/timeout results in no-op passthrough (the user's prompt
 * is admitted byte-identical).
 *
 * Retry-safety (per V2 docs): prompt hooks are not an exactly-once boundary — a retry
 * of an already-admitted prompt ID does not re-run hooks, and only the first successful
 * admission wins. A `<knocode_context>` marker guard additionally keeps the enrichment
 * idempotent for any replays that DO re-run the hook on an already-enriched draft.
 */

import { Plugin } from "@opencode-ai/plugin";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Helpers (exported for unit testing)
// ---------------------------------------------------------------------------

/**
 * Wait until the daemon reports ready via `GET /health` (parity with the UDS Probe).
 *
 * The HTTP health/metrics listener binds BEFORE the initial index, so during a cold
 * start `/health` answers `{"state": "indexing"}` and enrichment calls fail — polling here means the first real request gets context
 * instead of an instant passthrough.
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
  opts: { url?: string; timeoutMs?: number; pollMs?: number; fetchImpl?: typeof fetch } = {},
): Promise<boolean> {
  const url = opts.url ?? getDaemonUrl();
  const timeoutMs = opts.timeoutMs ?? getReadyTimeoutMs();
  const pollMs = opts.pollMs ?? DEFAULT_READY_POLL_MS;
  const fetchFn = opts.fetchImpl ?? fetch;
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    const remaining = deadline - Date.now();
    try {
      const res = await fetchFn(`${url}/health`, {
        // Per-poll cap (≤2s): a local daemon answers in ms; a stall means it is not healthy.
        signal: AbortSignal.timeout(Math.min(2_000, Math.max(1, remaining))),
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

// ---------------------------------------------------------------------------
// MCP client — JSON-RPC 2.0 over POST /mcp
// ---------------------------------------------------------------------------
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
  opts?: { url?: string; timeoutMs?: number; fetchImpl?: typeof fetch },
): Promise<McpCallOutcome> {
  const url = opts?.url ?? getDaemonUrl();
  const timeoutMs = opts?.timeoutMs ?? getTimeoutMs();
  const fetchFn = opts?.fetchImpl ?? fetch;
  const id = ++mcpRequestId;

  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), timeoutMs);

    const res = await fetchFn(`${url}/mcp`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
      signal: controller.signal,
    });

    clearTimeout(timeout);

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

/**
 * Successful enrichment outcome, with the metadata needed for the plugin's
 * integration-boundary metrics line (latency is measured by the caller).
 */
export type ContextEnrichment = {
  /** Full replacement text for the user prompt (original text preserved as prefix). */
  enrichedText: string;
  /** Reported token size of the injected context pack (daemon `total_tokens`). */
  tokens: number;
  /** Number of file provenance entries in the context pack. */
  files: number;
};

/**
 * Pre-model context enrichment: drive `knocode_context` via MCP. Returns the enriched
 * text (+ pack metadata) to substitute for the user prompt, or `null` for an untouched
 * passthrough (no context hits, indexing in progress, daemon down, or a daemon
 * without /mcp).
 */
export async function requestContextEnrichment(
  message: string,
  repositoryPath: string,
  opts?: { url?: string; timeoutMs?: number; fetchImpl?: typeof fetch },
): Promise<ContextEnrichment | null> {
  const out = await mcpCall(
    "tools/call",
    {
      name: "knocode_context",
      arguments: { prompt: message, repository_path: repositoryPath },
    },
    opts,
  );

  if (out.kind !== "ok") {
    // error (e.g. -32001 while indexing), unsupported (no /mcp), or failure —
    // untouched passthrough.
    return null;
  }

  const text: string | undefined = out.result?.content?.[0]?.text;
  const structured = out.result?.structuredContent ?? {};
  const passthrough = structured?.passthrough === true;
  const isError = out.result?.isError === true;
  if (!text || passthrough || isError) return null;
  return {
    enrichedText: text,
    tokens: typeof structured?.total_tokens === "number" ? structured.total_tokens : 0,
    files: Array.isArray(structured?.provenance) ? structured.provenance.length : 0,
  };
}

// ---------------------------------------------------------------------------
// V2 plugin — Plugin.define + session.prompt hook
// ---------------------------------------------------------------------------

/** Marker appended by the daemon context pack; used for idempotency on hook replays. */
export const CONTEXT_MARKER = "<knocode_context>";

/**
 * Resolve the agent workspace root from the V2 plugin context.
 *
 * TASK-036/F-7: sent with EVERY enrichment request so ONE shared daemon serves
 * multiple opencode windows on different repos simultaneously. `project.canonical`
 * is the canonical project root per the V2 docs; directory is the plugin location.
 */
export function resolveRepositoryPath(ctx: any): string {
  const location = ctx?.location ?? {};
  return (
    location?.project?.canonical ||
    location?.directory ||
    location?.project?.directory ||
    process.cwd()
  );
}

export const KnocodePlugin = Plugin.define({
  id: "opencode-knocode",

  async setup(ctx: any) {
    const repositoryPath = resolveRepositoryPath(ctx);

    // --- MCP Initialize --------------------------------------------------
    // Formal MCP handshake: initialize + notifications/initialized. This verifies
    // the daemon speaks MCP and retrieves protocol version and capabilities.
    // Fail-open: if daemon is unreachable, we proceed without initialization —
    // the prompt hook still attempts enrichment per call.
    async function initializeMcp(): Promise<void> {
      const out = await mcpCall(
        "initialize",
        {
          protocolVersion: "2024-11-05",
          capabilities: {},
          clientInfo: { name: "opencode-knocode", version: "0.9.11" },
        },
      );

      if (out.kind === "ok") {
        console.log(`[knocode] MCP initialized (daemon v${out.result?.serverInfo?.version || "unknown"})`);

        // Send initialized notification
        await mcpCall("notifications/initialized", {});
      } else if (out.kind === "unsupported") {
        console.log("[knocode] Daemon does not expose MCP — enrichment will passthrough");
      } else {
        console.log(`[knocode] MCP init failed: ${out.kind === "error" ? out.message : out.reason}`);
      }
    }

    // Try to initialize MCP on startup (non-blocking, fail-open)
    initializeMcp().catch(() => {});

    console.log(`[knocode] Plugin initialized (repository: ${repositoryPath})`);

    // --- Readiness gate --------------------------------------------------
    // The HTTP health/metrics listener binds BEFORE the initial index, so the daemon
    // answers `/health` with `state: "indexing"` (and 503s /hook-style enrichment)
    // during a cold start. We wait for readiness once, then skip re-polling for a cooldown window so
    // the check never adds latency to every message. Results are cached unconditionally:
    // once the daemon is actually ready a skipped gate doesn't matter (the POST succeeds
    // on its own), and a mid-index daemon only costs the wait budget once per window.
    const READY_REPOLL_MS = 30_000;
    let readinessCheckedAt = 0;

    async function ensureDaemonReady(): Promise<void> {
      const now = Date.now();
      if (now - readinessCheckedAt < READY_REPOLL_MS) return;
      const waitStartedAt = Date.now();
      const ready = await waitForDaemonReady();
      readinessCheckedAt = Date.now();
      if (ready && Date.now() - waitStartedAt > 1_000) {
        console.log(`[knocode] Daemon became ready after ${Date.now() - waitStartedAt}ms`);
      }
    }

    // --- session.prompt hook ---------------------------------------------
    // Runs once during prompt admission (before attachments/skills/inbox). Mutating
    // `event.prompt.text` makes the enriched text the canonical persisted user input.
    // Fail-open: passthrough leaves the draft untouched.
    await ctx.session.hook("prompt", async (event: any) => {
      const text: string | undefined = event?.prompt?.text;
      if (!text || text.trim().length === 0) return;

      // Idempotency guard: a replayed admission that re-runs this hook on an
      // already-enriched draft must not stack a second context block.
      if (text.includes(CONTEXT_MARKER)) return;

      // First real request of a session may hit a daemon mid-index (cold start or
      // auto-reindex); wait (bounded, fail-open) so this prompt gets context.
      await ensureDaemonReady();

      // Pre-model enrichment over the daemon MCP surface (typed tool call, no prompt
      // conversions). One Date.now() pair — the plugin's integration-boundary metric.
      const startedAt = Date.now();
      const result = await requestContextEnrichment(text, repositoryPath);
      const latencyMs = Date.now() - startedAt;

      if (result == null) {
        // Passthrough (no_context_hits / indexing / unreachable): leave the user's
        // prompt byte-identical — no metadata-only rewrite.
        console.log(`[knocode] context passthrough latency=${latencyMs}ms`);
        return;
      }

      event.prompt.text = result.enrichedText;

      // V2 docs: "When rewriting text, update or remove attachment mention offsets
      // that no longer match." The daemon preserves the original text as a prefix;
      // only when that invariant breaks do the file-mention offsets go stale.
      if (!result.enrichedText.startsWith(text) && Array.isArray(event.prompt.files)) {
        for (const file of event.prompt.files) {
          delete (file as any)?.mention;
        }
      }

      // The single integration-boundary metrics line: "Knocode added N ms to this
      // prompt" — latency is plugin cost, tokens/files are pack size and breadth.
      console.log(`[knocode] context latency=${latencyMs}ms tokens=${result.tokens} files=${result.files}`);
    });
  },
});

// Default export for opencode auto-discovery (V2 loads the default export's setup)
export default KnocodePlugin;
