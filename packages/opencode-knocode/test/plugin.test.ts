import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  getDaemonUrl,
  getTimeoutMs,
  getReadyTimeoutMs,
  waitForDaemonReady,
  mcpCall,
  requestContextEnrichment,
  resolveRepositoryPath,
  DEFAULT_DAEMON_URL,
  DEFAULT_TIMEOUT_MS,
  DEFAULT_READY_TIMEOUT_MS,
} from "../src/index";

describe("getDaemonUrl / getTimeoutMs", () => {
  const origEnv = { ...process.env };

  afterEach(() => {
    process.env = { ...origEnv };
  });

  it("returns default when env not set", () => {
    delete process.env.KNOCODE_DAEMON_URL;
    expect(getDaemonUrl()).toBe(DEFAULT_DAEMON_URL);
  });

  it("respects KNOCODE_DAEMON_URL", () => {
    process.env.KNOCODE_DAEMON_URL = "http://example:9999";
    expect(getDaemonUrl()).toBe("http://example:9999");
  });

  it("returns default timeout when not set", () => {
    delete process.env.KNOCODE_TIMEOUT_MS;
    expect(getTimeoutMs()).toBe(DEFAULT_TIMEOUT_MS);
  });

  it("parses KNOCODE_TIMEOUT_MS", () => {
    process.env.KNOCODE_TIMEOUT_MS = "5000";
    expect(getTimeoutMs()).toBe(5000);
  });

  it("falls back on invalid timeout", () => {
    process.env.KNOCODE_TIMEOUT_MS = "not-a-number";
    expect(getTimeoutMs()).toBe(DEFAULT_TIMEOUT_MS);
  });
});

describe("waitForDaemonReady", () => {
  beforeEach(() => vi.restoreAllMocks());

  const healthUrl = "http://127.0.0.1:9527/health";
  const okJson = (state: string | undefined) =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: async () => ({ status: "ok", state, index_files: 42 }),
    } as any);

  it("returns true immediately when /health reports ready", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okJson("ready"));
    const ready = await waitForDaemonReady({
      url: "http://127.0.0.1:9527",
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(ready).toBe(true);
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(mockFetch).toHaveBeenCalledWith(healthUrl, expect.objectContaining({ signal: expect.anything() }));
  });

  it("treats a 200 without a parseable state as ready (live daemon)", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okJson(undefined));
    const ready = await waitForDaemonReady({
      url: "http://127.0.0.1:9527",
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
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
    const ready = await waitForDaemonReady({
      url: "http://127.0.0.1:9527",
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(ready).toBe(true);
  });

  it("polls while indexing and returns true once ready", async () => {
    const responses = [okJson("indexing"), okJson("indexing"), okJson("ready")];
    const mockFetch = vi.fn().mockImplementation(async () => responses.shift());
    const ready = await waitForDaemonReady({
      url: "http://127.0.0.1:9527",
      timeoutMs: 1000,
      pollMs: 10,
      fetchImpl: mockFetch as any,
    });
    expect(ready).toBe(true);
    expect(mockFetch).toHaveBeenCalledTimes(3);
  });

  it("returns false when the budget expires while still indexing", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okJson("indexing"));
    const ready = await waitForDaemonReady({
      url: "http://127.0.0.1:9527",
      timeoutMs: 60,
      pollMs: 10,
      fetchImpl: mockFetch as any,
    });
    expect(ready).toBe(false);
    // ~6 polls in 60ms — proves it did not hang past the deadline
    expect(mockFetch.mock.calls.length).toBeGreaterThanOrEqual(4);
    expect(mockFetch.mock.calls.length).toBeLessThanOrEqual(10);
  });

  it("returns false fast when the daemon is unreachable (connection refused)", async () => {
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const t0 = Date.now();
    const ready = await waitForDaemonReady({
      url: "http://127.0.0.1:9527",
      timeoutMs: 10_000, // generous budget: must NOT be burned on a down daemon
      fetchImpl: mockFetch as any,
    });
    expect(ready).toBe(false);
    expect(Date.now() - t0).toBeLessThan(500);
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});

describe("getReadyTimeoutMs", () => {
  const origEnv = { ...process.env };

  afterEach(() => {
    process.env = { ...origEnv };
  });

  it("returns default when env not set", () => {
    delete process.env.KNOCODE_READY_TIMEOUT_MS;
    expect(getReadyTimeoutMs()).toBe(DEFAULT_READY_TIMEOUT_MS);
  });

  it("parses KNOCODE_READY_TIMEOUT_MS", () => {
    process.env.KNOCODE_READY_TIMEOUT_MS = "5000";
    expect(getReadyTimeoutMs()).toBe(5000);
  });

  it("falls back on invalid value", () => {
    process.env.KNOCODE_READY_TIMEOUT_MS = "nope";
    expect(getReadyTimeoutMs()).toBe(DEFAULT_READY_TIMEOUT_MS);
  });
});

describe("requestContextEnrichment argument wiring", () => {
  beforeEach(() => vi.restoreAllMocks());

  it("sends prompt + repository_path to knocode_context, no session id needed", async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({
        jsonrpc: "2.0",
        id: 1,
        result: { content: [{ type: "text", text: "enriched" }], structuredContent: { passthrough: false } },
      }),
    } as any);

    const enriched = await requestContextEnrichment("implement auth", "/repo", {
      url: "http://127.0.0.1:9527",
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });

    expect(enriched?.enrichedText).toBe("enriched");
    const body = JSON.parse(mockFetch.mock.calls[0][1].body);
    expect(body.params.name).toBe("knocode_context");
    expect(body.params.arguments.prompt).toBe("implement auth");
    expect(body.params.arguments.repository_path).toBe("/repo");
  });
});

describe("KnocodePlugin (V2 spec: Plugin.define + session.prompt)", () => {
  beforeEach(() => vi.restoreAllMocks());
  afterEach(() => vi.unstubAllGlobals());

  /** Boot the plugin and capture the registered prompt-hook callback. */
  async function makePromptHook(): Promise<{ hook: (event: any) => Promise<void> }> {
    const { KnocodePlugin } = await import("../src/index");
    const hooks: Array<(event: any) => Promise<void>> = [];
    await KnocodePlugin.setup({
      location: { directory: "/tmp", project: { canonical: "/repo/canonical" } },
      session: {
        hook: async (_name: string, cb: any) => {
          hooks.push(cb);
          return { dispose: async () => {} };
        },
      },
    } as any);
    expect(hooks.length).toBe(1);
    return { hook: hooks[0] };
  }

  /**
   * Stub global fetch for the full plugin lifecycle:
   *  - `/health` → ready (readiness gate)
   *  - `/mcp` initialize/notifications (setup handshake) → generic ok
   *  - `/mcp` tools/call (in-hook enrichment) → the given JSON-RPC result
   */
  function stubDaemonFetch(mcpResult: any) {
    return vi.fn().mockImplementation(async (url: any, init?: any) => {
      if (String(url).includes("/health")) {
        return { ok: true, status: 200, json: async () => ({ status: "ok", state: "ready" }) };
      }
      const method = JSON.parse(init?.body ?? "{}").method;
      if (method === "initialize" || method === "notifications/initialized") {
        return {
          ok: true,
          status: 200,
          json: async () => ({ jsonrpc: "2.0", id: 1, result: { serverInfo: { version: "0.0.0" } } }),
        };
      }
      return {
        ok: true,
        status: 200,
        json: async () => ({ jsonrpc: "2.0", id: 1, result: mcpResult }),
      };
    });
  }

  it("registers the session.prompt hook during setup", async () => {
    vi.stubGlobal("fetch", stubDaemonFetch({ content: [] }));
    const { hook } = await makePromptHook();
    expect(typeof hook).toBe("function");
  });

  it("mutates event.prompt.text with the daemon context (canonical persisted input)", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n<knocode_context>\nctx\n</knocode_context>" }],
        structuredContent: { type: "context", passthrough: false },
        isError: false,
      }),
    );
    const { hook } = await makePromptHook();

    const event: any = { sessionID: "s1", prompt: { text: "implement auth" } };
    await hook(event);

    expect(event.prompt.text).toContain("<knocode_context>");
    expect(event.prompt.text.startsWith("implement auth")).toBe(true);
  });

  it("sends the canonical project root as repository_path (TASK-036)", async () => {
    const fetchMock = vi.fn().mockImplementation(async (url: any, init?: any) => {
      if (String(url).includes("/health")) {
        return { ok: true, status: 200, json: async () => ({ status: "ok", state: "ready" }) };
      }
      const method = JSON.parse(init?.body ?? "{}").method;
      if (method === "initialize" || method === "notifications/initialized") {
        return {
          ok: true,
          status: 200,
          json: async () => ({ jsonrpc: "2.0", id: 1, result: { serverInfo: { version: "0.0.0" } } }),
        };
      }
      return {
        ok: true,
        status: 200,
        json: async () => ({
          jsonrpc: "2.0",
          id: 1,
          result: { content: [{ type: "text", text: "x" }], structuredContent: { passthrough: true } },
        }),
      };
    });
    vi.stubGlobal("fetch", fetchMock);
    const { hook } = await makePromptHook();

    await hook({ sessionID: "s1", prompt: { text: "hello" } });

    const toolsCall = fetchMock.mock.calls.find((c: any[]) => {
      try {
        return String(c[0]).includes("/mcp") && JSON.parse(c[1]?.body ?? "{}").method === "tools/call";
      } catch {
        return false;
      }
    });
    const body = JSON.parse(toolsCall![1].body);
    expect(body.params.arguments.repository_path).toBe("/repo/canonical");
  });

  it("passes through untouched on daemon passthrough (no_context_hits)", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "ctx" }],
        structuredContent: { passthrough: true, reason: "no_context_hits" },
        isError: false,
      }),
    );
    const { hook } = await makePromptHook();

    const event: any = { sessionID: "s1", prompt: { text: "unrelated prompt" } };
    await hook(event);

    expect(event.prompt.text).toBe("unrelated prompt");
  });

  it("is idempotent: does not re-enrich a draft that already has context", async () => {
    const fetchMock = vi.fn().mockImplementation(async (url: any, init?: any) => {
      if (String(url).includes("/health")) {
        return { ok: true, status: 200, json: async () => ({ status: "ok", state: "ready" }) };
      }
      return {
        ok: true,
        status: 200,
        json: async () => ({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "ctx" }] } }),
      };
    });
    vi.stubGlobal("fetch", fetchMock);
    const { hook } = await makePromptHook();

    const event: any = {
      sessionID: "s1",
      prompt: { text: "implement auth\n\n<knocode_context>\nctx\n</knocode_context>" },
    };
    await hook(event);

    expect(event.prompt.text).toBe("implement auth\n\n<knocode_context>\nctx\n</knocode_context>");
    const toolsCalls = fetchMock.mock.calls.filter((c: any[]) => {
      try {
        return String(c[0]).includes("/mcp") && JSON.parse(c[1]?.body ?? "{}").method === "tools/call";
      } catch {
        return false;
      }
    });
    expect(toolsCalls).toHaveLength(0);
  });

  it("stale attachment mention offsets are cleared when the rewrite breaks the prefix", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "totally rewritten" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const { hook } = await makePromptHook();

    const file: any = { uri: "file:///repo/a.ts", mention: { start: 0, end: 4, text: "auth" } };
    const event: any = { sessionID: "s1", prompt: { text: "look at auth", files: [file] } };
    await hook(event);

    expect(event.prompt.files[0].mention).toBeUndefined();
  });

  it("preserves attachment mentions when the rewrite keeps the original text as prefix", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "look at auth\n\n<knocode_context>\nctx\n</knocode_context>" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const { hook } = await makePromptHook();

    const mention = { start: 8, end: 12, text: "auth" };
    const event: any = {
      sessionID: "s1",
      prompt: { text: "look at auth", files: [{ uri: "file:///repo/a.ts", mention: { ...mention } }] },
    };
    await hook(event);

    expect(event.prompt.files[0].mention).toEqual(mention);
  });

  it("skips empty/whitespace prompts without calling the daemon", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const { hook } = await makePromptHook();
    fetchMock.mockClear(); // setup's fire-and-forget MCP initialize may have raced ahead

    const event: any = { sessionID: "s1", prompt: { text: "   " } };
    await hook(event);

    expect(event.prompt.text).toBe("   ");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("fails open when the daemon is unreachable", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("ECONNREFUSED")));
    const { hook } = await makePromptHook();

    const event: any = { sessionID: "s1", prompt: { text: "implement auth" } };
    await hook(event);

    expect(event.prompt.text).toBe("implement auth");
    errSpy.mockRestore();
  });
});

describe("resolveRepositoryPath (V2 ctx.location precedence)", () => {
  it("prefers project.canonical", () => {
    expect(
      resolveRepositoryPath({ location: { directory: "/a", project: { canonical: "/b", directory: "/c" } } }),
    ).toBe("/b");
  });

  it("falls back to location.directory, then cwd", () => {
    expect(resolveRepositoryPath({ location: { directory: "/a" } })).toBe("/a");
    expect(resolveRepositoryPath({})).toBe(process.cwd());
  });
});

describe("mcpCall (JSON-RPC over POST /mcp)", () => {
  beforeEach(() => vi.restoreAllMocks());

  const mcpUrl = "http://127.0.0.1:9527/mcp";
  const okResponse = (result: any) =>
    Promise.resolve({ ok: true, status: 200, json: async () => ({ jsonrpc: "2.0", id: 1, result }) } as any);

  it("returns result on success and posts a JSON-RPC envelope", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => okResponse({ content: [{ type: "text", text: "ctx" }] }));
    const out = await mcpCall("tools/call", { name: "knocode_context", arguments: { prompt: "hi" } }, {
      url: "http://127.0.0.1:9527",
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
    expect(mockFetch).toHaveBeenCalledWith(mcpUrl, expect.objectContaining({ method: "POST" }));
  });

  it("reports unsupported on 404 (legacy daemon without /mcp)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({ ok: false, status: 404 } as any);
    const out = await mcpCall("ping", {}, { url: "http://127.0.0.1:9527", fetchImpl: mockFetch as any });
    expect(out.kind).toBe("unsupported");
  });

  it("reports application JSON-RPC errors (e.g. -32001 daemon_indexing)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    } as any);
    const out = await mcpCall("tools/call", {}, { url: "http://127.0.0.1:9527", fetchImpl: mockFetch as any });
    expect(out.kind).toBe("error");
    if (out.kind === "error") expect(out.code).toBe(-32001);
  });

  it("fails open on network error", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const out = await mcpCall("ping", {}, { url: "http://127.0.0.1:9527", fetchImpl: mockFetch as any });
    expect(out.kind).toBe("failure");
    errSpy.mockRestore();
  });
});

describe("requestContextEnrichment (MCP knocode_context)", () => {
  beforeEach(() => vi.restoreAllMocks());

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

  it("returns enriched text + pack metadata (tokens/files) from knocode_context", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpToolResult(
        "implement auth\n\n---\n\nContext:\ncode_context: auth",
        {
          type: "context",
          passthrough: false,
          total_tokens: 2140,
          provenance: [
            { path: "src/a.rs", score: 0.9 },
            { path: "src/b.rs", score: 0.8 },
          ],
        },
      ),
    );
    const result = await requestContextEnrichment("implement auth", "/repo", {
      url: "http://127.0.0.1:9527",
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });
    expect(result?.enrichedText).toContain("Context:");
    expect(result?.tokens).toBe(2140);
    expect(result?.files).toBe(2);
    const [, init] = mockFetch.mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.params.name).toBe("knocode_context");
    expect(body.params.arguments.repository_path).toBe("/repo");
  });

  it("defaults tokens/files to 0 when structuredContent metadata is absent", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => mcpToolResult("ctx text"));
    const result = await requestContextEnrichment("hi", "/repo", { fetchImpl: mockFetch as any });
    expect(result?.enrichedText).toBe("ctx text");
    expect(result?.tokens).toBe(0);
    expect(result?.files).toBe(0);
  });

  it("returns null on daemon passthrough (zero context hits)", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpToolResult("unrelated", { type: "context", passthrough: true, reason: "no_context_hits" }),
    );
    const enriched = await requestContextEnrichment("unrelated", "/repo", {
      fetchImpl: mockFetch as any,
    });
    expect(enriched).toBeNull();
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it("returns null on -32001 indexing error", async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    } as any);
    const enriched = await requestContextEnrichment("hi", "/repo", { fetchImpl: mockFetch as any });
    expect(enriched).toBeNull();
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it("returns null on a daemon without /mcp (404)", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mockFetch = vi.fn().mockResolvedValue({ ok: false, status: 404 } as any);
    const enriched = await requestContextEnrichment("implement auth", "/repo", {
      fetchImpl: mockFetch as any,
    });
    expect(enriched).toBeNull();
    expect(mockFetch).toHaveBeenCalledTimes(1);
    errSpy.mockRestore();
  });
});

// requestOutputCompression describe block removed: the plugin no longer compresses —
// RTK (github.com/rtk-ai/rtk) owns the tool-output/command-rewrite layer.
