/**
 * Knocode daemon client for the `@knocode` Copilot chat participant.
 *
 * Thin host adapter over the SHARED daemon client, vendored from
 * `packages/knocode-client/src/client.ts` by every build/test/typecheck script
 * (`../../scripts/vendor-client.mjs`); the vendored copy is COMMITTED, and CI
 * runs the script with `--check` so drift fails the release build. Edit
 * wire-contract behavior in the source file, not in the vendored copy.
 *
 * The only host-specific behavior lives here: this extension defaults to a manual
 * `AbortController` timeout because `AbortSignal.timeout` can crash libuv on
 * Windows when combined with immediate process teardown. Callers can override it
 * per call via `opts.timeoutFactory`.
 *
 * Fail-open contract (unchanged): any error, timeout, indexing-in-progress, or
 * zero-hit returns a tagged passthrough outcome so the participant always runs
 * with the bare prompt.
 */
import {
  type ClientOptions,
  type ContextOutcome,
  requestContextEnrichment as sharedRequestContextEnrichment,
} from "./vendor/knocode-client";

export {
  DEFAULT_DAEMON_URL,
  DEFAULT_READY_POLL_MS,
  DEFAULT_READY_TIMEOUT_MS,
  DEFAULT_TIMEOUT_MS,
  ensureDaemonReady,
  getDaemonUrl,
  getReadyTimeoutMs,
  getTimeoutMs,
  mcpCall,
  newRequestId,
  resetReadinessCache,
  waitForDaemonReady,
} from "./vendor/knocode-client";
export type {
  ClientOptions,
  ContextEnrichment,
  ContextOutcome,
  ContextPassthrough,
  KnocodeFetchResponse,
  McpCallOutcome,
  TimeoutFactory,
} from "./vendor/knocode-client";

/**
 * Host-default timeout strategy — see module doc. The timer is unref'd so a
 * pending timeout never delays host (extension host / vitest) teardown.
 */
function manualTimeoutFactory(ms: number): AbortSignal {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), ms);
  (timer as any).unref?.();
  return controller.signal;
}

/**
 * Pre-model context enrichment (shared implementation, host timeout default).
 * Returns the enriched text (+ pack metadata), or a tagged passthrough outcome
 * (no context hits, indexing in progress, daemon down, or a daemon without /mcp)
 * — the caller runs with the bare prompt in that case.
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
  return sharedRequestContextEnrichment(message, repositoryPath, {
    timeoutFactory: manualTimeoutFactory,
    ...opts,
  });
}
