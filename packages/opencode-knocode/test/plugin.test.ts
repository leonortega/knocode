import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { resolveRepositoryPath, server as v1Server } from "../src/index";

// Client contract tests (config env vars, readiness gate, mcpCall envelope,
// requestContextEnrichment outcomes) live in packages/knocode-client/test —
// the single source of truth this plugin vendors at build time. The tests here
// cover the plugin integration itself (V1 chat.message + V2 session.prompt wiring).

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

  it("registers the session.prompt hook during setup", async () => {
    vi.stubGlobal("fetch", stubDaemonFetch({ content: [] }));
    const { hook } = await makePromptHook();
    expect(typeof hook).toBe("function");
  });

  it("mutates event.prompt.text with the daemon context (canonical persisted input)", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { type: "context", passthrough: false },
        isError: false,
      }),
    );
    const { hook } = await makePromptHook();

    const event: any = { sessionID: "s1", prompt: { text: "implement auth" } };
    await hook(event);

    expect(event.prompt.text).toContain("\n\n---\n\nContext:\n");
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
      prompt: { text: "implement auth\n\n---\n\nContext:\ncode_context: auth" },
    };
    await hook(event);

    expect(event.prompt.text).toBe("implement auth\n\n---\n\nContext:\ncode_context: auth");
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
        content: [{ type: "text", text: "look at auth\n\n---\n\nContext:\ncode_context: auth" }],
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
    // Full-lifecycle stub so the fire-and-forget setup handshake doesn't hit
    // an undefined-response mock and spam stderr (the bare vi.fn() used to).
    const fetchMock = stubDaemonFetch({ content: [] });
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

describe("V1 server() (OpenCode 1.x: server() + chat.message)", () => {
  beforeEach(() => vi.restoreAllMocks());
  afterEach(() => vi.unstubAllGlobals());

  /** Boot the V1 plugin and capture its Hooks object. */
  async function makeV1Hooks(input: any = { worktree: "/repo/worktree" }): Promise<Record<string, any>> {
    vi.stubGlobal("fetch", stubDaemonFetch({ content: [] }));
    const hooks = await v1Server(input);
    expect(typeof hooks["chat.message"]).toBe("function");
    return hooks;
  }

  it("exposes a chat.message hook from server()", async () => {
    const hooks = await makeV1Hooks();
    expect(typeof hooks["chat.message"]).toBe("function");
  });

  it("enriches a user message in place (string content)", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const msg: any = { role: "user", content: "implement auth" };
    await hooks["chat.message"]({ message: msg, sessionID: "s1" }, {});

    expect(msg.content).toContain("\n\n---\n\nContext:\n");
    expect(msg.content.startsWith("implement auth")).toBe(true);
  });

  it("enriches array-part content by replacing text parts", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const msg: any = { role: "user", content: [{ type: "text", text: "implement auth" }] };
    await hooks["chat.message"]({ message: msg, sessionID: "s1" }, {});

    expect(Array.isArray(msg.content)).toBe(true);
    expect(msg.content).toHaveLength(1);
    expect(msg.content[0].text).toContain("\n\n---\n\nContext:\n");
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
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const msg: any = { role: "user", content: "unrelated prompt" };
    await hooks["chat.message"]({ message: msg, sessionID: "s1" }, {});

    expect(msg.content).toBe("unrelated prompt");
  });

  it("is idempotent: does not re-enrich an already-enriched message", async () => {
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
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const msg: any = { role: "user", content: "already\n\n---\n\nContext:\ncode_context: auth" };
    await hooks["chat.message"]({ message: msg, sessionID: "s1" }, {});

    expect(msg.content).toBe("already\n\n---\n\nContext:\ncode_context: auth");
    const toolsCalls = fetchMock.mock.calls.filter((c: any[]) => {
      try {
        return String(c[0]).includes("/mcp") && JSON.parse(c[1]?.body ?? "{}").method === "tools/call";
      } catch {
        return false;
      }
    });
    expect(toolsCalls).toHaveLength(0);
  });

  it("skips non-user roles and empty text", async () => {
    // Full-lifecycle stub so the fire-and-forget setup handshake doesn't hit
    // an undefined-response mock and spam stderr (the bare vi.fn() used to).
    const fetchMock = stubDaemonFetch({ content: [] });
    vi.stubGlobal("fetch", fetchMock);
    const hooks = await v1Server({ worktree: "/repo/worktree" });
    // Let the fire-and-forget setup handshake (initialize + notifications) settle
    // before clearing, so its two calls don't race the assertion below.
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));
    fetchMock.mockClear();

    const assistant: any = { role: "assistant", content: "hi" };
    await hooks["chat.message"]({ message: assistant, sessionID: "s1" }, {});
    expect(assistant.content).toBe("hi");

    const empty: any = { role: "user", content: "   " };
    await hooks["chat.message"]({ message: empty, sessionID: "s1" }, {});
    expect(empty.content).toBe("   ");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("fails open when the daemon is unreachable", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("ECONNREFUSED")));
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const msg: any = { role: "user", content: "implement auth" };
    await hooks["chat.message"]({ message: msg, sessionID: "s1" }, {});

    expect(msg.content).toBe("implement auth");
    errSpy.mockRestore();
  });

  it("sends the V1 worktree as repository_path (TASK-036)", async () => {
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
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const msg: any = { role: "user", content: "hello" };
    await hooks["chat.message"]({ message: msg, sessionID: "s1" }, {});

    const toolsCall = fetchMock.mock.calls.find((c: any[]) => {
      try {
        return String(c[0]).includes("/mcp") && JSON.parse(c[1]?.body ?? "{}").method === "tools/call";
      } catch {
        return false;
      }
    });
    const body = JSON.parse(toolsCall![1].body);
    expect(body.params.arguments.repository_path).toBe("/repo/worktree");
  });
});

describe("default export (dual V1 + V2 entrypoint)", () => {
  it("carries both the V2 definition and the V1 server() function", async () => {
    const mod = await import("../src/index");
    expect((mod.default as any).id).toBe("opencode-knocode");
    expect(typeof (mod.default as any).setup).toBe("function");
    expect(typeof (mod.default as any).server).toBe("function");
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
