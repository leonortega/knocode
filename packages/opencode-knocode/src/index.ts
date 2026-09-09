/**
 * Knocode AI Runtime Plugin for OpenCode (V1 + V2 plugin specs)
 *
 * OpenCode V2 plugin API (@opencode-ai/plugin "beta", Plugin.define + ctx.session.hook):
 * the `session.prompt` hook intercepts the incoming user prompt BEFORE attachment and
 * skill resolution and durable inbox admission, and receives an owned, mutable draft
 * (`prompt.text`, `prompt.files`, `metadata`, `delivery`). Edits become the canonical
 * persisted user input for that admission.
 *
 * OpenCode V1 plugin API (@opencode-ai/plugin 1.x, server() + hooks): the `chat.message`
 * hook receives the user message before dispatch and mutates `message.content` in place.
 * V1 and V2 are served from ONE entrypoint per the documented "Support V1" pattern:
 * the default export spreads Plugin.define({ id, setup }) AND carries a server() function;
 * V1 calls server(), V2 reads id/setup() and ignores server().
 *
 * Knocode uses both to enrich the prompt with repository context from the daemon
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
 * admission wins. A delimiter guard additionally keeps the enrichment idempotent for
 * any replays that DO re-run the hook on an already-enriched draft.
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
  logAtVerbosity,
} from "./vendor/knocode-client.js";

// ---------------------------------------------------------------------------
// Shared enrichment helpers (used by both the V1 and V2 hook paths)
// ---------------------------------------------------------------------------

/**
 * Delimiter the daemon emits between the original prompt and the context YAML
 * (http_server.rs: `format!("{}\n\n---\n\nContext:\n{}", message, yaml)`) — i.e.
 * `enrichedText` is a FULL replacement whose prefix is the original prompt. Used for
 * idempotency on hook replays: a draft already carrying the delimiter is left alone.
 * (Never invent a marker the daemon doesn't emit — the guard must match the real
 * wire format or it can never fire against production output.)
 */
export const CONTEXT_DELIMITER = "\n\n---\n\nContext:\n";

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

/**
 * Shared MCP initialize handshake (V1 and V2 paths run the same one-shot startup).
 */
function createInitializeMcp(): () => Promise<void> {
  return async function initializeMcp(): Promise<void> {
    const out = await mcpCall(
      "initialize",
      {
        protocolVersion: "2024-11-05",
        capabilities: {},
        clientInfo: { name: "opencode-knocode", version: "0.9.11" },
      },
    );

    if (out.kind === "ok") {
      // Verbose only: per-call detail lives at verbosity 2.
      logAtVerbosity(2, `[knocode] MCP initialized (daemon v${out.result?.serverInfo?.version || "unknown"})`);

      // Send initialized notification
      await mcpCall("notifications/initialized", {});
    } else if (out.kind === "unsupported") {
      logAtVerbosity(1, "[knocode] Daemon does not expose MCP — enrichment will passthrough");
    } else {
      logAtVerbosity(1, `[knocode] MCP init failed: ${out.kind === "error" ? out.message : out.reason}`);
    }
  };
}

/**
 * Fire the startup handshake without blocking or failing setup (fail-open).
 */
function startupMcpHandshake(): void {
  createInitializeMcp()().catch(() => {});
}

/**
 * Run enrichment for one prompt text and log the outcome at the integration
 * boundary. Returns the enriched text, or undefined on passthrough (caller must
 * leave the user's prompt byte-identical).
 */
async function enrichPromptText(
  text: string,
  repositoryPath: string,
): Promise<string | undefined> {
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
    logAtVerbosity(1, `[knocode] context passthrough request_id=${result.requestId} reason=${result.reason} latency=${latencyMs}ms`);
    return undefined;
  }

  // The single integration-boundary metrics line: "Knocode added N ms to this
  // prompt" — latency is plugin cost, tokens/files are pack size and breadth,
  // request_id correlates with the daemon's "MCP knocode_context built" line.
  logAtVerbosity(1, `[knocode] context request_id=${result.requestId} latency=${latencyMs}ms tokens=${result.tokens} files=${result.files}`);

  // Verbosity 2: show the payload the agent will actually receive — first 400
  // chars, newlines escaped (mirrors the daemon's "context payload ready" line).
  const total = result.enrichedText.length;
  const preview = result.enrichedText.slice(0, 400).replace(/\r?\n/g, "\\n");
  logAtVerbosity(2, `[knocode] payload (${total} chars): ${preview}${total > 400 ? `… (+${total - 400} chars)` : ""}`);

  return result.enrichedText;
}

// ---------------------------------------------------------------------------
// V2 plugin — Plugin.define + session.prompt hook
// ---------------------------------------------------------------------------

export const KnocodePlugin = Plugin.define({
  id: "opencode-knocode",

  async setup(ctx: any) {
    const repositoryPath = resolveRepositoryPath(ctx);

    // Non-blocking, fail-open startup handshake (shared with the V1 path).
    startupMcpHandshake();

    logAtVerbosity(1, `[knocode] Plugin initialized (repository: ${repositoryPath})`);

    // --- session.prompt hook ---------------------------------------------
    // Runs once during prompt admission (before attachments/skills/inbox). Mutating
    // `event.prompt.text` makes the enriched text the canonical persisted user input.
    // Fail-open: passthrough leaves the draft untouched.
    await ctx.session.hook("prompt", async (event: any) => {
      const text: string | undefined = event?.prompt?.text;
      if (!text || text.trim().length === 0) return;

      // Idempotency guard: a replayed admission that re-runs this hook on an
      // already-enriched draft must not stack a second context block. A prompt that
      // naturally contains the daemon delimiter would skip enrichment too — the
      // fail-open direction (no double-stacked context beats a missing one).
      if (text.includes(CONTEXT_DELIMITER)) return;

      const enriched = await enrichPromptText(text, repositoryPath);
      if (enriched === undefined) return;

      event.prompt.text = enriched;

      // V2 docs: "When rewriting text, update or remove attachment mention offsets
      // that no longer match." The daemon preserves the original text as a prefix;
      // only when that invariant breaks do the file-mention offsets go stale.
      if (!enriched.startsWith(text) && Array.isArray(event.prompt.files)) {
        for (const file of event.prompt.files) {
          delete (file as any)?.mention;
        }
      }
    });
  },
});

// ---------------------------------------------------------------------------
// V1 plugin — server() + chat.message hook (OpenCode 1.x)
// ---------------------------------------------------------------------------

/**
 * Extract the prompt text from a V1 message. `message.content` may be a plain
 * string or an array of parts (text parts joined with newlines).
 */
function extractMessageText(msg: any): string | undefined {
  if (typeof msg?.content === "string") return msg.content;
  if (Array.isArray(msg?.content)) {
    const text = msg.content
      .filter((p: any) => p?.type === "text")
      .map((p: any) => p.text)
      .join("\n");
    return text.length > 0 ? text : undefined;
  }
  return undefined;
}

/**
 * V1 server() hook factory — returns the V1 Hooks object OpenCode 1.x expects
 * from the default export. Runs the SAME enrichment path as the V2 hook: shared
 * vendored client, idempotency marker, fail-open passthrough, metric lines.
 */
export async function server(_input?: any): Promise<Record<string, any>> {
  // TASK-036/F-7: the agent's active workspace root — sent with EVERY request so ONE
  // shared daemon serves multiple opencode windows on different repos. V1 gives the
  // plugin factory a worktree + directory; project.worktree is the historical fallback.
  const input = _input ?? {};
  const repositoryPath: string =
    input.worktree || input.directory || input.project?.worktree || process.cwd();

  // Non-blocking, fail-open startup handshake (shared with the V2 path).
  startupMcpHandshake();

  logAtVerbosity(1, `[knocode] Plugin initialized (repository: ${repositoryPath})`);

  return {
    // Called when a new message is received — mutate message.content in place;
    // OpenCode reads it after the hook returns.
    "chat.message": async (input: any, _output: any) => {
      const msg = input?.message;
      if (!msg || msg.role !== "user") return;

      const text = extractMessageText(msg);
      if (!text || text.trim().length === 0) return;

      // Idempotency guard: the same delimiter guard as the V2 hook.
      if (text.includes(CONTEXT_DELIMITER)) return;

      const enriched = await enrichPromptText(text, repositoryPath);
      if (enriched === undefined) return;

      // Mutate in place — opencode reads input.message after the hook.
      if (typeof msg.content === "string") {
        msg.content = enriched;
      } else if (Array.isArray(msg.content)) {
        // Replace text parts with a single enriched part.
        msg.content = [{ type: "text", text: enriched }];
      }
    },
  };
}

// ---------------------------------------------------------------------------
// Entrypoint — V1 and V2 from ONE default export (documented "Support V1" shape)
// ---------------------------------------------------------------------------

// V1 reads the default export's server() and calls it with its PluginInput;
// V2 reads the default export's id + setup() and ignores server().
// KnocodePlugin itself is also exported (named) for direct/testing use.
const KnocodePluginWithV1 = {
  ...KnocodePlugin,
  server,
};

// Default export for opencode auto-discovery (V1: server(); V2: id + setup()).
export default KnocodePluginWithV1;
