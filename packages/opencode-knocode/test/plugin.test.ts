import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { resolveRepositoryPath } from "../src/index";

// Client contract tests (config env vars, readiness gate, mcpCall envelope,
// requestContextEnrichment outcomes) live in packages/knocode-client/test —
// the single source of truth this plugin vendors at build time. The tests here
// cover the plugin integration itself (V2 session.prompt hook wiring).

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

// requestOutputCompression describe block removed: the plugin no longer compresses —
// RTK (github.com/rtk-ai/rtk) owns the tool-output/command-rewrite layer.
