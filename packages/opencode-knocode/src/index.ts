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
 * hook signature is `(input:{sessionID...}, output:{message:UserMessage, parts:Part[]})`
 * and mutates `output.parts` in place (text lives in text parts, not `message.content`).
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
import path from "node:path";

// ---------------------------------------------------------------------------
// Vendored daemon client (single source of truth: packages/knocode-client)
// ---------------------------------------------------------------------------

export * from "./vendor/knocode-client.js";

import {
  mcpCall,
  requestContextEnrichment,
  ensureDaemonReady,
  logAtVerbosity,
  getVerbosity,
  type Verbosity,
} from "./vendor/knocode-client.js";

export type ServerLogLevel = "debug" | "info" | "warn" | "error";

/**
 * Plugin log sink. `console.log` from a plugin goes to the host server's
 * stdout, which is invisible in OpenCode Desktop — so when the V1 `client`
 * is available, lines are sent via `client.app.log()` (structured server
 * logs) with `console.log` as the fallback. Respects the shared
 * `KNOCODE_LOG_LEVEL` verbosity gate, same as `logAtVerbosity`.
 */
export type ServerLogger = (verbosity: Verbosity, level: ServerLogLevel, message: string) => void;

export function createServerLogger(client: any): ServerLogger {
  return (verbosity, level, message) => {
    if (getVerbosity() < verbosity) return;
    const log = client?.app?.log;
    if (typeof log === "function") {
      try {
        const pending = log.call(client.app, { body: { service: "knocode", level, message } });
        if (pending && typeof pending.catch === "function") {
          pending.catch(() => {
            console.log(message);
          });
        }
      } catch {
        console.log(message);
      }
    } else {
      console.log(message);
    }
  };
}

/** Default sink: plain `console.log` (V2 path and tests without a client). */
export const consoleServerLogger: ServerLogger = (verbosity, _level, message) => {
  logAtVerbosity(verbosity, message);
};

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
 * NOTE: `ctx.location` is the PLUGIN INSTANCE location (where the plugin was
 * loaded), explicitly NOT the location of every session it can access or event
 * it receives (per the V2 plugin docs). It is only a setup-time FALLBACK.
 * The per-prompt session directory is resolved in `resolveEventRepositoryPath`
 * via `ctx.session.get({ sessionID })` — that is what actually scopes retrieval
 * to the right repo when one daemon serves many windows/repos.
 *
 * Kept as the setup-time fallback (and exported for tests).
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
 * Unwrap an OpenCode session payload. `ctx.session.get` returns Session.Info
 * directly in most client versions, but tolerate `{ data }` wrappers.
 */
function unwrapSession(session: any): any {
  if (session && typeof session === "object" && "data" in session && session.data && typeof session.data === "object") {
    return (session as any).data;
  }
  return session;
}

/**
 * Extract the workspace root from a Session.Info payload (V2 `session.get`):
 * `location.directory` (+ optional `subpath`), with legacy shapes tolerated.
 */
export function extractSessionDirectory(session: any): string | undefined {
  const s = unwrapSession(session);
  if (!s || typeof s !== "object") return undefined;
  const dir = s?.location?.directory;
  if (typeof dir === "string" && dir.trim().length > 0) {
    const sub = s?.subpath;
    if (typeof sub === "string" && sub.trim().length > 0 && sub !== ".") {
      try {
        return path.join(dir, sub);
      } catch {
        return dir;
      }
    }
    return dir;
  }
  const legacy =
    s?.directory ?? s?.worktree ?? s?.project?.canonical ?? s?.project?.directory;
  if (typeof legacy === "string" && legacy.trim().length > 0) return legacy;
  return undefined;
}

/**
 * Per-prompt repository resolution (V2): the prompt hook event carries
 * `sessionID` — look up THAT session's working directory so each window/repo
 * gets its own context. Fail-open: any lookup failure returns `fallback`
 * (the setup-time `resolveRepositoryPath`), never throws.
 */
export async function resolveEventRepositoryPath(
  ctx: any,
  event: any,
  fallback: string,
  log: ServerLogger = consoleServerLogger,
): Promise<string> {
  const sessionID = event?.sessionID ?? event?.sessionId;
  const get = ctx?.session?.get;
  if (sessionID && typeof get === "function") {
    try {
      const session = await get.call(ctx.session, { sessionID });
      const dir = extractSessionDirectory(session);
      if (dir) {
        log(2, "debug", `[knocode] repo resolved from session ${sessionID}: ${dir}`);
        return dir;
      }
      log(2, "debug", `[knocode] session ${sessionID} has no directory — using fallback ${fallback}`);
    } catch (e) {
      // Fall through to the setup-time fallback (fail-open).
      log(2, "debug", `[knocode] session lookup failed (${String(e)}) — using fallback ${fallback}`);
    }
  } else if (sessionID) {
    log(2, "debug", `[knocode] no session getter available — using fallback ${fallback}`);
  }
  // The hook input itself occasionally carries location hints (defensive).
  const hinted =
    event?.directory ?? event?.worktree ?? event?.project?.canonical ?? event?.project?.directory;
  if (typeof hinted === "string" && hinted.trim().length > 0) return hinted;
  return fallback;
}

/**
 * Call a V1 `client.session.get` across its known shapes. The 1.x SDK is
 * hey-api generated: `get({ path: { id } })` → `{ data: Session, ... }`.
 * Older/promise-style clients take `{ sessionID }` or the raw id. Tries each
 * in order, returns the first non-null response, throws the last error when
 * all shapes fail.
 */
async function callV1SessionGet(get: (...args: any[]) => Promise<any>, ctx: any, sessionID: string): Promise<any> {
  const attempts: Array<() => Promise<any>> = [
    () => get.call(ctx, { path: { id: sessionID } }),
    () => get.call(ctx, { sessionID }),
    () => get.call(ctx, sessionID),
  ];
  let lastError: unknown = new Error("session lookup failed");
  for (const attempt of attempts) {
    try {
      const res = await attempt();
      if (res !== undefined && res !== null) return res;
    } catch (e) {
      lastError = e;
    }
  }
  throw lastError;
}

/**
 * Per-message repository resolution (V1): the `chat.message` hook input carries
 * `sessionID` — best-effort lookup through the V1 `client.session.get` when
 * present, else hook-input hints, else the `server()`-time fallback. Fail-open:
 * never throws.
 */
export async function resolveV1MessageRepositoryPath(
  client: any,
  hookInput: any,
  fallback: string,
  log: ServerLogger = consoleServerLogger,
): Promise<string> {
  const sessionID = hookInput?.sessionID ?? hookInput?.sessionId;
  const get = client?.session?.get;
  if (sessionID && typeof get === "function") {
    try {
      const session = await callV1SessionGet(get, client.session, sessionID);
      const dir = extractSessionDirectory(session);
      if (dir) {
        log(2, "debug", `[knocode] repo resolved from session ${sessionID}: ${dir}`);
        return dir;
      }
      log(2, "debug", `[knocode] session ${sessionID} has no directory — using fallback ${fallback}`);
    } catch (e) {
      // Fall through (fail-open).
      log(2, "debug", `[knocode] session lookup failed (${String(e)}) — using fallback ${fallback}`);
    }
  } else if (sessionID) {
    log(2, "debug", `[knocode] no client session getter available — using fallback ${fallback}`);
  }
  const hinted =
    hookInput?.directory ??
    hookInput?.worktree ??
    hookInput?.session?.location?.directory ??
    hookInput?.session?.directory ??
    hookInput?.project?.worktree ??
    hookInput?.project?.directory;
  if (typeof hinted === "string" && hinted.trim().length > 0) return hinted;
  return fallback;
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
  log: ServerLogger = consoleServerLogger,
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
    // `repo=` shows which workspace the daemon was asked to scope retrieval to.
    log(1, "info", `[knocode] context passthrough request_id=${result.requestId} reason=${result.reason} repo=${repositoryPath} latency=${latencyMs}ms`);
    return undefined;
  }

  // The single integration-boundary metrics line: "Knocode added N ms to this
  // prompt" — latency is plugin cost, tokens/files are pack size and breadth,
  // request_id correlates with the daemon's "MCP knocode_context built" line.
  log(1, "info", `[knocode] context request_id=${result.requestId} latency=${latencyMs}ms tokens=${result.tokens} files=${result.files} repo=${repositoryPath}`);

  // Verbosity 2: show the payload the agent will actually receive — first 400
  // chars, newlines escaped (mirrors the daemon's "context payload ready" line).
  const total = result.enrichedText.length;
  const preview = result.enrichedText.slice(0, 400).replace(/\r?\n/g, "\\n");
  log(2, "debug", `[knocode] payload (${total} chars): ${preview}${total > 400 ? `… (+${total - 400} chars)` : ""}`);

  return result.enrichedText;
}

// ---------------------------------------------------------------------------
// V2 plugin — Plugin.define + session.prompt hook
// ---------------------------------------------------------------------------

export const KnocodePlugin = Plugin.define({
  id: "opencode-knocode",

  async setup(ctx: any) {
    // Setup-time fallback ONLY (plugin-instance location, not per-session).
    const setupRepositoryPath = resolveRepositoryPath(ctx);

    // One-time shape probe: if per-session lookups ever fail, this line shows
    // what the plugin host gave us at load (paths only, no secrets).
    try {
      const loc = ctx?.location ?? {};
      logAtVerbosity(
        1,
        `[knocode] setup location directory=${loc?.directory} project.directory=${loc?.project?.directory} project.canonical=${loc?.project?.canonical} cwd=${process.cwd()}`,
      );
    } catch {
      // Logging must never break setup.
    }

    // Non-blocking, fail-open startup handshake (shared with the V1 path).
    startupMcpHandshake();

    logAtVerbosity(1, `[knocode] Plugin initialized (repository fallback: ${setupRepositoryPath})`);

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

      // Per-prompt session directory (multi-repo): the setup-time fallback only
      // applies when the session lookup is unavailable.
      const repositoryPath = await resolveEventRepositoryPath(ctx, event, setupRepositoryPath);
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
 * Extract the prompt text from V1 `output.parts` (stable `@opencode-ai/plugin`
 * 1.x: `chat.message(input:{sessionID...}, output:{message:UserMessage,
 * parts:Part[]})`). Text lives in `parts[]` (`type:"text"`, field `.text`);
 * `UserMessage` itself carries no `content` field.
 */
function extractPartsText(parts: any): string | undefined {
  if (!Array.isArray(parts)) return undefined;
  const text = parts
    .filter((p: any) => p?.type === "text" && typeof p?.text === "string")
    .map((p: any) => p.text)
    .join("\n");
  return text.length > 0 ? text : undefined;
}

/**
 * Replace text parts in place with the enriched text. The first text part keeps
 * its identity (id/sessionID/messageID) so downstream part references stay
 * valid; remaining text parts are dropped; non-text parts (files, etc.) are
 * preserved in order.
 */
function replacePartsText(parts: any[], enriched: string): void {
  const firstTextIdx = parts.findIndex((p: any) => p?.type === "text");
  if (firstTextIdx === -1) return;
  const first = parts[firstTextIdx];
  const next: any[] = [];
  for (let i = 0; i < parts.length; i++) {
    if (i === firstTextIdx) {
      next.push({ ...first, text: enriched });
    } else if (parts[i]?.type === "text") {
      continue;
    } else {
      next.push(parts[i]);
    }
  }
  parts.length = 0;
  parts.push(...next);
}

/**
 * V1 server() hook factory — returns the V1 Hooks object OpenCode 1.x expects
 * from the default export. Runs the SAME enrichment path as the V2 hook: shared
 * vendored client, idempotency marker, fail-open passthrough, metric lines.
 */
export async function server(_input?: any): Promise<Record<string, any>> {
  // server()-time fallback ONLY (plugin-load location, not per-session). The
  // per-message session directory is resolved inside the hook below — required
  // for OpenCode Desktop, where one loaded plugin serves many sessions/repos.
  const input = _input ?? {};
  const setupRepositoryPath: string =
    input.worktree || input.directory || input.project?.worktree || process.cwd();

  // Structured server logs via client.app.log() (console.log is invisible in
  // OpenCode Desktop) with console fallback when no client is present.
  const log = createServerLogger(input.client);

  // Non-blocking, fail-open startup handshake (shared with the V2 path).
  startupMcpHandshake();

  log(1, "info", `[knocode] Plugin initialized (repository fallback: ${setupRepositoryPath})`);

  // One-time shape probe (V1): shows what the host passed at load so a wrong
  // fallback (e.g. drive root) can be traced to its source. Paths only.
  try {
    const keys = _input && typeof _input === "object" ? Object.keys(_input).join(",") : typeof _input;
    log(
      1,
      "info",
      `[knocode] server input keys=${keys} worktree=${input.worktree} directory=${input.directory} project=${JSON.stringify(input.project)} client.session.get=${typeof input.client?.session?.get} cwd=${process.cwd()}`,
    );
  } catch {
    // Logging must never break setup.
  }

  // Keep the V1 client for per-message session lookups (best-effort, fail-open).
  const v1Client = input.client;

  return {
    // Called when a new message is received — mutate output.parts in place;
    // OpenCode reads it after the hook returns.
    "chat.message": async (hookInput: any, output: any) => {
      const parts = output?.parts;
      if (!Array.isArray(parts)) return;
      // Guard: only enrich user turns (UserMessage.role === "user").
      if (output?.message && output.message.role && output.message.role !== "user") return;

      const text = extractPartsText(parts);
      if (!text || text.trim().length === 0) return;

      // Idempotency guard: the same delimiter guard as the V2 hook.
      if (text.includes(CONTEXT_DELIMITER)) return;

      // Per-message session directory (multi-repo); falls back to the
      // server()-time path when the session lookup is unavailable.
      const repositoryPath = await resolveV1MessageRepositoryPath(v1Client, hookInput, setupRepositoryPath, log);
      const enriched = await enrichPromptText(text, repositoryPath, log);
      if (enriched === undefined) return;

      // Mutate in place — opencode reads output.parts after the hook.
      replacePartsText(parts, enriched);
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
