import { describe, it, expect, vi, afterEach } from "vitest";
import {
  getDaemonUrl,
  getTimeoutMs,
  mcpCall,
  requestContextEnrichment,
} from "../src/daemon";

const URL = "http://127.0.0.1:9527";

const jsonResponse = (body: any) =>
  ({ ok: true, status: 200, json: async () => body } as any);

describe("getDaemonUrl / getTimeoutMs", () => {
  const orig = { ...process.env };
  afterEach(() => {
    process.env = { ...orig };
  });

  it("uses the default daemon URL", () => {
    delete process.env.KNOCODE_DAEMON_URL;
    expect(getDaemonUrl()).toBe("http://127.0.0.1:9527");
  });

  it("respects KNOCODE_DAEMON_URL", () => {
    process.env.KNOCODE_DAEMON_URL = "http://example:9999";
    expect(getDaemonUrl()).toBe("http://example:9999");
  });

  it("parses KNOCODE_TIMEOUT_MS and falls back on invalid", () => {
    process.env.KNOCODE_TIMEOUT_MS = "5000";
    expect(getTimeoutMs()).toBe(5000);
    process.env.KNOCODE_TIMEOUT_MS = "nope";
    expect(getTimeoutMs()).toBe(30_000);
  });
});

describe("mcpCall", () => {
  afterEach(() => vi.restoreAllMocks());

  it("returns the result on success (knocode_context)", async () => {
    const mockFetch = vi.fn().mockResolvedValue(
      jsonResponse({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "ctx" }] } }),
    );
    const out = await mcpCall(
      "tools/call",
      { name: "knocode_context", arguments: { prompt: "hi" } },
      { url: URL, timeoutMs: 1000, fetchImpl: mockFetch as any },
    );
    expect(out.kind).toBe("ok");
    if (out.kind === "ok") {
      expect(out.result.content[0].text).toBe("ctx");
    }
    const body = JSON.parse(mockFetch.mock.calls[0][1].body);
    expect(body.method).toBe("tools/call");
    expect(body.params.name).toBe("knocode_context");
  });

  it("returns an error envelope for JSON-RPC application errors (e.g. -32001 indexing)", async () => {
    const mockFetch = vi.fn().mockResolvedValue(
      jsonResponse({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    );
    const out = await mcpCall("tools/call", {}, { url: URL, timeoutMs: 1000, fetchImpl: mockFetch as any });
    expect(out.kind).toBe("error");
  });

  it("is unsupported for legacy daemons (404 /mcp)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({ ok: false, status: 404 } as any);
    const out = await mcpCall("tools/call", {}, { url: URL, timeoutMs: 1000, fetchImpl: mockFetch as any });
    expect(out.kind).toBe("unsupported");
  });

  it("fails open when unreachable", async () => {
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const out = await mcpCall("tools/call", {}, { url: URL, timeoutMs: 1000, fetchImpl: mockFetch as any });
    expect(out.kind).toBe("failure");
  });
});

describe("requestContextEnrichment", () => {
  afterEach(() => vi.restoreAllMocks());

  it("returns enriched outcome on success and forwards repository_path + request_id", async () => {
    const mockFetch = vi.fn().mockResolvedValue(
      jsonResponse({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "repo digest" }], structuredContent: { total_tokens: 2140, provenance: [{ path: "a.ts" }] }, isError: false } }),
    );
    const outcome = await requestContextEnrichment("implement auth", "C:/repo", {
      url: URL,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(outcome.kind).toBe("enriched");
    if (outcome.kind === "enriched") {
      expect(outcome.enrichedText).toBe("repo digest");
      expect(outcome.tokens).toBe(2140);
      expect(outcome.files).toBe(1);
    }
    const body = JSON.parse(mockFetch.mock.calls[0][1].body);
    expect(body.params.arguments.repository_path).toBe("C:/repo");
    // §7 request correlation: a client-generated UUID travels with every call
    expect(body.params.arguments.request_id).toMatch(/^[0-9a-f-]{36}$/);
    if (outcome.kind === "enriched") {
      expect(body.params.arguments.request_id).toBe(outcome.requestId);
    }
  });

  it("uses opts.requestId when provided instead of generating one", async () => {
    const mockFetch = vi.fn().mockResolvedValue(
      jsonResponse({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "ctx" }], structuredContent: {}, isError: false } }),
    );
    const outcome = await requestContextEnrichment("hi", "C:/repo", {
      url: URL,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
      requestId: "fixed-id-for-test",
    });
    expect(outcome.kind).toBe("enriched");
    const body = JSON.parse(mockFetch.mock.calls[0][1].body);
    expect(body.params.arguments.request_id).toBe("fixed-id-for-test");
  });

  it("returns tagged passthrough when the daemon is unreachable (fail-open)", async () => {
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const outcome = await requestContextEnrichment("hi", "C:/repo", {
      url: URL,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(outcome).toEqual({ kind: "passthrough", reason: "daemon_unreachable", requestId: expect.any(String) });
  });

  it("returns tagged passthrough carrying the daemon reason (zero context hits)", async () => {
    const mockFetch = vi.fn().mockResolvedValue(
      jsonResponse({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "x" }], structuredContent: { passthrough: true, reason: "no_context_hits" }, isError: false } }),
    );
    const outcome = await requestContextEnrichment("hi", "C:/repo", {
      url: URL,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(outcome).toEqual({ kind: "passthrough", reason: "no_context_hits", requestId: expect.any(String) });
  });

  it("classifies an unspecified passthrough when the daemon sends no reason", async () => {
    const mockFetch = vi.fn().mockResolvedValue(
      jsonResponse({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "x" }], structuredContent: { passthrough: true }, isError: false } }),
    );
    const outcome = await requestContextEnrichment("hi", "C:/repo", {
      url: URL,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(outcome.kind).toBe("passthrough");
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("unspecified");
  });

  it("returns tagged passthrough with mcp_error_<code> when the daemon is mid-index (-32001)", async () => {
    const mockFetch = vi.fn().mockResolvedValue(
      jsonResponse({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    );
    const outcome = await requestContextEnrichment("hi", "C:/repo", {
      url: URL,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(outcome.kind).toBe("passthrough");
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("mcp_error_-32001");
  });

  it("returns tagged passthrough with no_mcp_surface for legacy daemons (404)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({ ok: false, status: 404 } as any);
    const outcome = await requestContextEnrichment("hi", "C:/repo", {
      url: URL,
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(outcome.kind).toBe("passthrough");
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("no_mcp_surface");
  });
});