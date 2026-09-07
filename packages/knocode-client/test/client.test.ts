import { describe, it, expect, vi, afterEach } from "vitest";
import {
  getDaemonUrl,
  getTimeoutMs,
  getReadyTimeoutMs,
  mcpCall,
  waitForDaemonReady,
  requestContextEnrichment,
  newRequestId,
  DEFAULT_DAEMON_URL,
  DEFAULT_TIMEOUT_MS,
  DEFAULT_READY_TIMEOUT_MS,
} from "../src/client.js";
import * as client from "../src/client.js";

const URL_ = "http://127.0.0.1:9527";

const okResponse = (result: any) =>
  Promise.resolve({ ok: true, status: 200, json: async () => ({ jsonrpc: "2.0", id: 1, result }) } as any);

describe("getDaemonUrl / getTimeoutMs / getReadyTimeoutMs", () => {
  const origEnv = { ...process.env };
  afterEach(() => {
    process.env = { ...origEnv };
  });

  it("returns default when env not set", () => {
    delete process.env.KNOCODE_DAEMON_URL;
    delete process.env.KNOCODE_TIMEOUT_MS;
    delete process.env.KNOCODE_READY_TIMEOUT_MS;
    expect(getDaemonUrl()).toBe(DEFAULT_DAEMON_URL);
    expect(getTimeoutMs()).toBe(DEFAULT_TIMEOUT_MS);
    expect(getReadyTimeoutMs()).toBe(DEFAULT_READY_TIMEOUT_MS);
  });

  it("respects env overrides", () => {
    process.env.KNOCODE_DAEMON_URL = "http://example:9999";
    process.env.KNOCODE_TIMEOUT_MS = "5000";
    process.env.KNOCODE_READY_TIMEOUT_MS = "5000";
    expect(getDaemonUrl()).toBe("http://example:9999");
    expect(getTimeoutMs()).toBe(5000);
    expect(getReadyTimeoutMs()).toBe(5000);
  });

  it("falls back on invalid values", () => {
    process.env.KNOCODE_TIMEOUT_MS = "not-a-number";
    process.env.KNOCODE_READY_TIMEOUT_MS = "nope";
    expect(getTimeoutMs()).toBe(DEFAULT_TIMEOUT_MS);
    expect(getReadyTimeoutMs()).toBe(DEFAULT_READY_TIMEOUT_MS);
  });
});

describe("mcpCall (JSON-RPC over POST /mcp)", () => {
  afterEach(() => vi.restoreAllMocks());

  it("returns result on success and posts a JSON-RPC envelope", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okResponse({ content: [{ type: "text", text: "ctx" }] }));
    const out = await mcpCall("tools/call", { name: "knocode_context", arguments: { prompt: "hi" } }, {
      url: URL_,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(out.kind).toBe("ok");
    if (out.kind === "ok") expect(out.result.content[0].text).toBe("ctx");
    const [, init] = mockFetch.mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.jsonrpc).toBe("2.0");
    expect(body.method).toBe("tools/call");
    expect(body.id).toBeGreaterThan(0);
    expect(mockFetch).toHaveBeenCalledWith(`${URL_}/mcp`, expect.objectContaining({ method: "POST" }));
  });

  it("reports unsupported on 404 (legacy daemon without /mcp)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({ ok: false, status: 404 } as any);
    const out = await mcpCall("ping", {}, { url: URL_, fetchImpl: mockFetch as any });
    expect(out.kind).toBe("unsupported");
  });

  it("reports application JSON-RPC errors (e.g. -32001 daemon_indexing)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    } as any);
    const out = await mcpCall("tools/call", {}, { url: URL_, fetchImpl: mockFetch as any });
    expect(out.kind).toBe("error");
    if (out.kind === "error") expect(out.code).toBe(-32001);
  });

  it("fails open on network error", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const out = await mcpCall("ping", {}, { url: URL_, fetchImpl: mockFetch as any });
    expect(out.kind).toBe("failure");
    errSpy.mockRestore();
  });

  it("uses a host-provided timeoutFactory for the per-call signal", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okResponse({}));
    const timeoutFactory = vi.fn(() => AbortSignal.timeout(1000));
    await mcpCall("ping", {}, { url: URL_, timeoutMs: 1000, fetchImpl: mockFetch as any, timeoutFactory });
    expect(timeoutFactory).toHaveBeenCalledWith(1000);
  });
});

describe("waitForDaemonReady", () => {
  const healthUrl = `${URL_}/health`;
  const okJson = (state: string | undefined) =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: async () => ({ status: "ok", state, index_files: 42 }),
    } as any);

  afterEach(() => vi.restoreAllMocks());

  it("returns true immediately when /health reports ready", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okJson("ready"));
    const ready = await waitForDaemonReady({ url: URL_, timeoutMs: 1000, fetchImpl: mockFetch as any });
    expect(ready).toBe(true);
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(mockFetch).toHaveBeenCalledWith(healthUrl, expect.objectContaining({ signal: expect.anything() }));
  });

  it("treats a 200 without a parseable state as ready (live daemon)", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okJson(undefined));
    const ready = await waitForDaemonReady({ url: URL_, timeoutMs: 1000, fetchImpl: mockFetch as any });
    expect(ready).toBe(true);
  });

  it("treats a 200 with a non-JSON body as ready", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => ({
      ok: true,
      status: 200,
      json: async () => {
        throw new Error("not json");
      },
    } as any));
    const ready = await waitForDaemonReady({ url: URL_, timeoutMs: 1000, fetchImpl: mockFetch as any });
    expect(ready).toBe(true);
  });

  it("polls while indexing and returns true once ready", async () => {
    const responses = [okJson("indexing"), okJson("indexing"), okJson("ready")];
    const mockFetch = vi.fn().mockImplementation(async () => responses.shift());
    const ready = await waitForDaemonReady({ url: URL_, timeoutMs: 1000, pollMs: 10, fetchImpl: mockFetch as any });
    expect(ready).toBe(true);
    expect(mockFetch).toHaveBeenCalledTimes(3);
  });

  it("returns false when the budget expires while still indexing", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okJson("indexing"));
    const ready = await waitForDaemonReady({ url: URL_, timeoutMs: 60, pollMs: 10, fetchImpl: mockFetch as any });
    expect(ready).toBe(false);
    expect(mockFetch.mock.calls.length).toBeGreaterThanOrEqual(4);
    expect(mockFetch.mock.calls.length).toBeLessThanOrEqual(10);
  });

  it("returns false fast when the daemon is unreachable (connection refused)", async () => {
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const t0 = Date.now();
    const ready = await waitForDaemonReady({ url: URL_, timeoutMs: 10_000, fetchImpl: mockFetch as any });
    expect(ready).toBe(false);
    expect(Date.now() - t0).toBeLessThan(500);
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});

describe("ensureDaemonReady (readiness cooldown)", () => {
  afterEach(() => {
    client.resetReadinessCache();
    vi.restoreAllMocks();
  });

  it("waits once, then skips within the cooldown window", async () => {
    client.resetReadinessCache();
    const mockFetch = vi.fn().mockImplementation(async () =>
      Promise.resolve({ ok: true, status: 200, json: async () => ({ status: "ok", state: "ready" }) } as any),
    );
    await client.ensureDaemonReady({ url: URL_, timeoutMs: 1000, fetchImpl: mockFetch as any });
    await client.ensureDaemonReady({ url: URL_, timeoutMs: 1000, fetchImpl: mockFetch as any });
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});

describe("newRequestId", () => {
  it("generates unique correlation ids", () => {
    expect(newRequestId()).not.toBe(newRequestId());
    expect(newRequestId()).toMatch(/^[0-9a-f-]{36}$/);
  });
});

describe("requestContextEnrichment (MCP knocode_context)", () => {
  afterEach(() => vi.restoreAllMocks());

  const mcpToolResult = (text: string, structured: any = {}, isError = false) =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: async () => ({
        jsonrpc: "2.0",
        id: 1,
        result: { content: [{ type: "text", text }], structuredContent: structured, isError },
      }),
    } as any);

  it("returns enriched outcome + pack metadata from knocode_context", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpToolResult("implement auth\n\n---\n\nContext:\ncode_context: auth", {
        type: "context",
        passthrough: false,
        total_tokens: 2140,
        provenance: [
          { path: "src/a.rs", score: 0.9 },
          { path: "src/b.rs", score: 0.8 },
        ],
      }),
    );
    const result = await requestContextEnrichment("implement auth", "/repo", {
      url: URL_,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(result.kind).toBe("enriched");
    if (result.kind === "enriched") {
      expect(result.enrichedText).toContain("Context:");
      expect(result.tokens).toBe(2140);
      expect(result.files).toBe(2);
    }
    const [, init] = mockFetch.mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.params.name).toBe("knocode_context");
    expect(body.params.arguments.prompt).toBe("implement auth");
    expect(body.params.arguments.repository_path).toBe("/repo");
    // §7 request correlation: the generated requestId is the one sent to the daemon
    expect(body.params.arguments.request_id).toMatch(/^[0-9a-f-]{36}$/);
    if (result.kind === "enriched") expect(body.params.arguments.request_id).toBe(result.requestId);
  });

  it("omits repository_path when the host has none (direct API caller parity)", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => mcpToolResult("ctx"));
    const result = await requestContextEnrichment("hi", undefined, { fetchImpl: mockFetch as any });
    expect(result.kind).toBe("enriched");
    const body = JSON.parse((mockFetch.mock.calls[0] as any[])[1].body);
    expect(body.params.arguments.repository_path).toBeUndefined();
    expect(body.params.arguments.request_id).toBeDefined();
  });

  it("defaults tokens/files to 0 when structuredContent metadata is absent", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => mcpToolResult("ctx text"));
    const result = await requestContextEnrichment("hi", "/repo", { fetchImpl: mockFetch as any });
    expect(result.kind).toBe("enriched");
    if (result.kind === "enriched") {
      expect(result.tokens).toBe(0);
      expect(result.files).toBe(0);
    }
  });

  it("returns a tagged passthrough with the daemon reason on zero context hits", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpToolResult("unrelated", { type: "context", passthrough: true, reason: "no_context_hits" }),
    );
    const outcome = await requestContextEnrichment("unrelated", "/repo", { fetchImpl: mockFetch as any });
    expect(outcome).toEqual({ kind: "passthrough", reason: "no_context_hits", requestId: expect.any(String) });
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it("classifies an unspecified passthrough when the daemon sends no reason", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => mcpToolResult("x", { passthrough: true }));
    const outcome = await requestContextEnrichment("hi", "/repo", { fetchImpl: mockFetch as any });
    expect(outcome.kind).toBe("passthrough");
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("unspecified");
  });

  it("returns a tagged passthrough with mcp_error_<code> on -32001 indexing", async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    } as any);
    const outcome = await requestContextEnrichment("hi", "/repo", { fetchImpl: mockFetch as any });
    expect(outcome.kind).toBe("passthrough");
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("mcp_error_-32001");
  });

  it("returns a tagged passthrough with no_mcp_surface on a daemon without /mcp (404)", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mockFetch = vi.fn().mockResolvedValue({ ok: false, status: 404 } as any);
    const outcome = await requestContextEnrichment("implement auth", "/repo", { fetchImpl: mockFetch as any });
    expect(outcome.kind).toBe("passthrough");
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("no_mcp_surface");
    expect(mockFetch).toHaveBeenCalledTimes(1);
    errSpy.mockRestore();
  });

  it("returns a tagged passthrough with daemon_unreachable on transport failure (fail-open)", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const outcome = await requestContextEnrichment("implement auth", "/repo", { fetchImpl: mockFetch as any });
    expect(outcome).toEqual({ kind: "passthrough", reason: "daemon_unreachable", requestId: expect.any(String) });
    errSpy.mockRestore();
  });

  it("uses opts.requestId when provided instead of generating one", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => mcpToolResult("ctx"));
    const result = await requestContextEnrichment("hi", "/repo", {
      fetchImpl: mockFetch as any,
      requestId: "fixed-id-for-test",
    });
    expect(result.kind).toBe("enriched");
    const [, init] = mockFetch.mock.calls[0];
    expect(JSON.parse(init.body).params.arguments.request_id).toBe("fixed-id-for-test");
  });
});
