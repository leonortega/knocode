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
 * The daemon client (MCP transport, readiness gate, request correlation, tagged
 * context outcomes) is the shared single source of truth in
 * `packages/knocode-client/src/client.ts`, vendored into `src/vendor/` by every
 * build/test/typecheck script (`../../scripts/vendor-client.mjs`); the vendored
 * copy is COMMITTED, and CI runs the script with `--check` so drift fails the
 * release build. Edit client behavior THERE, not in the vendored copy.
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
// Vendored daemon client (single source of truth: packages/knocode-client)
// ---------------------------------------------------------------------------

export * from "./vendor/knocode-client.js";

import {
  mcpCall,
  requestContextEnrichment,
  ensureDaemonReady,
} from "./vendor/knocode-client.js";

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

      if (result.kind === "passthrough") {
        // Passthrough (no_context_hits / daemon_indexing / unreachable / error): leave
        // the user's prompt byte-identical — no metadata-only rewrite. The reason makes
        // this line self-sufficient: no daemon log access needed to classify it, and
        // request_id joins it with the daemon's log line for the same request.
        console.log(`[knocode] context passthrough request_id=${result.requestId} reason=${result.reason} latency=${latencyMs}ms`);
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
      // prompt" — latency is plugin cost, tokens/files are pack size and breadth,
      // request_id correlates with the daemon's "MCP knocode_context built" line.
      console.log(`[knocode] context request_id=${result.requestId} latency=${latencyMs}ms tokens=${result.tokens} files=${result.files}`);
    });
  },
});

// Default export for opencode auto-discovery (V2 loads the default export's setup)
export default KnocodePlugin;
