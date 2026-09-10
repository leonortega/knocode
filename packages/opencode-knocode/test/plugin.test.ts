import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  resolveRepositoryPath,
  extractSessionDirectory,
  resolveEventRepositoryPath,
  resolveV1MessageRepositoryPath,
  server as v1Server,
} from "../src/index";

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

  /** Boot the plugin and capture the registered prompt + context hook callbacks. */
  async function makePromptHook(): Promise<{
    hook: (event: any) => Promise<void>;
    promptHook: (event: any) => Promise<void>;
    contextHook: (event: any) => Promise<void>;
  }> {
    const { KnocodePlugin } = await import("../src/index");
    const byName = new Map<string, (event: any) => Promise<void>>();
    await KnocodePlugin.setup({
      location: { directory: "/tmp", project: { canonical: "/repo/canonical" } },
      session: {
        hook: async (name: string, cb: any) => {
          byName.set(name, cb);
          return { dispose: async () => {} };
        },
      },
    } as any);
    expect(byName.has("prompt")).toBe(true);
    expect(byName.has("context")).toBe(true);
    const promptHook = byName.get("prompt")!;
    // Legacy alias: existing tests call `hook(event)` for the prompt stage.
    return { hook: promptHook, promptHook, contextHook: byName.get("context")! };
  }

  it("registers the session.prompt hook during setup", async () => {
    vi.stubGlobal("fetch", stubDaemonFetch({ content: [] }));
    const { hook } = await makePromptHook();
    expect(typeof hook).toBe("function");
  });

  it("leaves prompt.text untouched and injects context via the context hook", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { type: "context", passthrough: false },
        isError: false,
      }),
    );
    const { promptHook, contextHook } = await makePromptHook();

    const event: any = { sessionID: "s1", prompt: { text: "implement auth" } };
    await promptHook(event);

    // Transcript stays exactly what the user typed.
    expect(event.prompt.text).toBe("implement auth");

    const ctxEvent: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent);

    expect(ctxEvent.system).toHaveLength(1);
    expect(ctxEvent.system[0].text).toContain("code_context: auth");
    expect(ctxEvent.system[0].text).toContain("<repository context>");

    // Consume-once: tool-driven continuations don't re-inject.
    const ctxEvent2: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent2);
    expect(ctxEvent2.system).toHaveLength(0);
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
    const { hook, contextHook } = await makePromptHook();

    const event: any = { sessionID: "s1", prompt: { text: "unrelated prompt" } };
    await hook(event);

    expect(event.prompt.text).toBe("unrelated prompt");
    const ctxEvent: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent);
    expect(ctxEvent.system).toHaveLength(0);
  });

  it("is idempotent: does not fetch when the draft already has context", async () => {
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
    const { hook, contextHook } = await makePromptHook();

    const event: any = {
      sessionID: "s1",
      prompt: { text: "implement auth\n\n---\n\nContext:\ncode_context: auth" },
    };
    await hook(event);

    expect(event.prompt.text).toBe("implement auth\n\n---\n\nContext:\ncode_context: auth");
    const ctxEvent: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent);
    expect(ctxEvent.system).toHaveLength(0);
    const toolsCalls = fetchMock.mock.calls.filter((c: any[]) => {
      try {
        return String(c[0]).includes("/mcp") && JSON.parse(c[1]?.body ?? "{}").method === "tools/call";
      } catch {
        return false;
      }
    });
    expect(toolsCalls).toHaveLength(0);
  });

  it("injects via context hook even when the rewrite breaks the prefix, prompt files untouched", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "totally rewritten" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const { hook, contextHook } = await makePromptHook();

    const file: any = { uri: "file:///repo/a.ts", mention: { start: 0, end: 4, text: "auth" } };
    const event: any = { sessionID: "s1", prompt: { text: "look at auth", files: [file] } };
    await hook(event);

    // Prompt never mutated, so attachment offsets stay valid.
    expect(event.prompt.text).toBe("look at auth");
    expect(event.prompt.files[0].mention).toEqual({ start: 0, end: 4, text: "auth" });

    const ctxEvent: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent);
    expect(ctxEvent.system).toHaveLength(1);
    expect(ctxEvent.system[0].text).toContain("totally rewritten");
  });

  it("preserves attachment mentions and injects stripped context when prefix holds", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "look at auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const { hook, contextHook } = await makePromptHook();

    const mention = { start: 8, end: 12, text: "auth" };
    const event: any = {
      sessionID: "s1",
      prompt: { text: "look at auth", files: [{ uri: "file:///repo/a.ts", mention: { ...mention } }] },
    };
    await hook(event);

    expect(event.prompt.text).toBe("look at auth");
    expect(event.prompt.files[0].mention).toEqual(mention);

    const ctxEvent: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent);
    expect(ctxEvent.system).toHaveLength(1);
    // Context-only strip: system carries the YAML, not the echoed prompt.
    expect(ctxEvent.system[0].text).toContain("code_context: auth");
    expect(ctxEvent.system[0].text).not.toContain("look at auth\n\n---");
  });

  it("skips empty/whitespace prompts without calling the daemon", async () => {
    // Full-lifecycle stub so the fire-and-forget setup handshake doesn't hit
    // an undefined-response mock and spam stderr (the bare vi.fn() used to).
    const fetchMock = stubDaemonFetch({ content: [] });
    vi.stubGlobal("fetch", fetchMock);
    const { hook, contextHook } = await makePromptHook();
    fetchMock.mockClear(); // setup's fire-and-forget MCP initialize may have raced ahead

    const event: any = { sessionID: "s1", prompt: { text: "   " } };
    await hook(event);

    expect(event.prompt.text).toBe("   ");
    expect(fetchMock).not.toHaveBeenCalled();
    const ctxEvent: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent);
    expect(ctxEvent.system).toHaveLength(0);
  });

  it("fails open when the daemon is unreachable", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("ECONNREFUSED")));
    const { hook, contextHook } = await makePromptHook();

    const event: any = { sessionID: "s1", prompt: { text: "implement auth" } };
    await hook(event);

    expect(event.prompt.text).toBe("implement auth");
    const ctxEvent: any = { sessionID: "s1", system: [] };
    await contextHook(ctxEvent);
    expect(ctxEvent.system).toHaveLength(0);
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

  it("enriches output.parts text in place (stable 1.x signature)", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const output: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "implement auth" }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, output);

    expect(output.parts).toHaveLength(1);
    expect(output.parts[0].text).toContain("\n\n---\n\nContext:\n");
    expect(output.parts[0].text.startsWith("implement auth")).toBe(true);
  });

  it("joins multiple text parts and preserves non-text parts", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { passthrough: false },
        isError: false,
      }),
    );
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const output: any = {
      message: { role: "user" },
      parts: [
        { type: "text", text: "implement" },
        { type: "file", path: "/repo/a.ts" },
        { type: "text", text: "auth" },
      ],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, output);

    const texts = output.parts.filter((p: any) => p?.type === "text");
    expect(texts).toHaveLength(1);
    expect(texts[0].text).toContain("\n\n---\n\nContext:\n");
    expect(output.parts.some((p: any) => p?.type === "file")).toBe(true);
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

    const output: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "unrelated prompt" }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, output);

    expect(output.parts[0].text).toBe("unrelated prompt");
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

    const output: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "already\n\n---\n\nContext:\ncode_context: auth" }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, output);

    expect(output.parts[0].text).toBe("already\n\n---\n\nContext:\ncode_context: auth");
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

    const assistantOut: any = {
      message: { role: "assistant" },
      parts: [{ type: "text", text: "hi" }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, assistantOut);
    expect(assistantOut.parts[0].text).toBe("hi");

    const emptyOut: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "   " }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, emptyOut);
    expect(emptyOut.parts[0].text).toBe("   ");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("fails open when the daemon is unreachable", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("ECONNREFUSED")));
    const hooks = await v1Server({ worktree: "/repo/worktree" });

    const output: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "implement auth" }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, output);

    expect(output.parts[0].text).toBe("implement auth");
    errSpy.mockRestore();
  });

  it("routes init + outcome lines through client.app.log when a client is present", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "implement auth\n\n---\n\nContext:\ncode_context: auth" }],
        structuredContent: { passthrough: false, total_tokens: 7, provenance: [{ path: "a" }] },
        isError: false,
      }),
    );
    const appLog = vi.fn().mockResolvedValue(true);
    const hooks = await v1Server({ worktree: "/repo/worktree", client: { app: { log: appLog } } });

    // Init line goes to the server logs, not stdout.
    expect(appLog).toHaveBeenCalledWith(
      expect.objectContaining({ body: expect.objectContaining({ service: "knocode", level: "info" }) }),
    );
    const initMsg = (appLog.mock.calls[0][0] as any).body.message as string;
    expect(initMsg).toContain("Plugin initialized");

    const output: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "implement auth" }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, output);

    expect(output.parts[0].text).toContain("\n\n---\n\nContext:\n");
    const messages = appLog.mock.calls.map((c: any[]) => (c[0] as any).body.message as string);
    expect(messages.some((m: string) => m.includes("context request_id="))).toBe(true);
  });

  it("falls back to console.log when no client is present", async () => {
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
    vi.stubGlobal("fetch", stubDaemonFetch({ content: [] }));
    await v1Server({ worktree: "/repo/worktree" });
    expect(logSpy).toHaveBeenCalledWith(expect.stringContaining("[knocode] Plugin initialized"));
    logSpy.mockRestore();
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

    const output: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "hello" }],
    };
    await hooks["chat.message"]({ sessionID: "s1" }, output);

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

describe("extractSessionDirectory (Session.Info shapes)", () => {
  it("reads location.directory", () => {
    expect(extractSessionDirectory({ location: { directory: "/repo/session" } })).toBe("/repo/session");
  });

  it("joins subpath onto the session directory", () => {
    const joined = extractSessionDirectory({
      location: { directory: "/repo" },
      subpath: "packages/app",
    });
    expect(joined).toContain("packages");
    expect(joined).toContain("app");
  });

  it("tolerates { data } wrappers and legacy shapes", () => {
    expect(extractSessionDirectory({ data: { location: { directory: "/wrapped" } } })).toBe("/wrapped");
    expect(extractSessionDirectory({ directory: "/legacy" })).toBe("/legacy");
    expect(extractSessionDirectory({ project: { canonical: "/proj" } })).toBe("/proj");
  });

  it("returns undefined when no directory is present", () => {
    expect(extractSessionDirectory({})).toBeUndefined();
    expect(extractSessionDirectory(undefined)).toBeUndefined();
  });
});

describe("resolveEventRepositoryPath (V2 per-prompt session lookup)", () => {
  it("prefers the session directory over the setup-time fallback", async () => {
    const ctx: any = {
      session: { get: async () => ({ location: { directory: "/sessions/mattermost" } }) },
    };
    await expect(
      resolveEventRepositoryPath(ctx, { sessionID: "s1" }, "/plugin/load-location"),
    ).resolves.toBe("/sessions/mattermost");
  });

  it("falls back when the session lookup throws", async () => {
    const ctx: any = {
      session: {
        get: async () => {
          throw new Error("gone");
        },
      },
    };
    await expect(resolveEventRepositoryPath(ctx, { sessionID: "s1" }, "/fallback")).resolves.toBe(
      "/fallback",
    );
  });

  it("falls back when there is no session getter", async () => {
    await expect(resolveEventRepositoryPath({}, { sessionID: "s1" }, "/fallback")).resolves.toBe(
      "/fallback",
    );
  });

  it("hook sends the SESSION directory even when setup location is a drive root", async () => {
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
    const { KnocodePlugin } = await import("../src/index");
    const byName = new Map<string, (event: any) => Promise<void>>();
    await KnocodePlugin.setup({
      // Simulates the reported bug: plugin instance loaded at drive root.
      location: { directory: "C:\\", project: { canonical: "C:\\", directory: "C:\\" } },
      session: {
        hook: async (name: string, cb: any) => {
          byName.set(name, cb);
          return { dispose: async () => {} };
        },
        get: async ({ sessionID }: any) => {
          expect(sessionID).toBe("sess-mattermost");
          return { location: { directory: "C:\\tmp\\mattermost-master" } };
        },
      },
    } as any);
    expect(byName.has("prompt")).toBe(true);
    expect(byName.has("context")).toBe(true);

    await byName.get("prompt")!({ sessionID: "sess-mattermost", prompt: { text: "what is the main class?" } });

    const toolsCall = fetchMock.mock.calls.find((c: any[]) => {
      try {
        return String(c[0]).includes("/mcp") && JSON.parse(c[1]?.body ?? "{}").method === "tools/call";
      } catch {
        return false;
      }
    });
    expect(toolsCall).toBeDefined();
    const body = JSON.parse(toolsCall![1].body);
    expect(body.params.arguments.repository_path).toBe("C:\\tmp\\mattermost-master");
  });
});

describe("resolveV1MessageRepositoryPath (V1 per-message lookup)", () => {
  it("prefers client.session.get over the server()-time fallback", async () => {
    const client: any = {
      session: { get: async () => ({ location: { directory: "/v1/session-repo" } }) },
    };
    await expect(
      resolveV1MessageRepositoryPath(client, { sessionID: "s1" }, "/server/fallback"),
    ).resolves.toBe("/v1/session-repo");
  });

  it("uses hook-input hints when no client lookup exists", async () => {
    await expect(
      resolveV1MessageRepositoryPath(undefined, { sessionID: "s1", worktree: "/hint/repo" }, "/fb"),
    ).resolves.toBe("/hint/repo");
  });

  it("calls the hey-api shape { path: { id } } and unwraps { data }", async () => {
    const seen: any[] = [];
    const client: any = {
      session: {
        get: async (args: any) => {
          seen.push(args);
          if (args?.path?.id) return { data: { directory: "C:\\tmp\\mattermost-master" } };
          throw new Error("bad request");
        },
      },
    };
    await expect(
      resolveV1MessageRepositoryPath(client, { sessionID: "sess-hey" }, "C:\\"),
    ).resolves.toBe("C:\\tmp\\mattermost-master");
    expect(seen[0]).toEqual({ path: { id: "sess-hey" } });
  });

  it("falls back when the lookup throws", async () => {
    const client: any = {
      session: {
        get: async () => {
          throw new Error("down");
        },
      },
    };
    await expect(
      resolveV1MessageRepositoryPath(client, { sessionID: "s1" }, "/fb"),
    ).resolves.toBe("/fb");
  });

  it("hook sends the per-message session directory, not the server() fallback", async () => {    const fetchMock = vi.fn().mockImplementation(async (url: any, init?: any) => {
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
    const hooks = await v1Server({
      worktree: "C:\\",
      client: {
        session: {
          get: async ({ sessionID }: any) => {
            expect(sessionID).toBe("sess-mm");
            return { location: { directory: "C:\\tmp\\mattermost-master" } };
          },
        },
      },
    });

    const output: any = {
      message: { role: "user" },
      parts: [{ type: "text", text: "what is the main class?" }],
    };
    await hooks["chat.message"]({ sessionID: "sess-mm" }, output);

    const toolsCall = fetchMock.mock.calls.find((c: any[]) => {
      try {
        return String(c[0]).includes("/mcp") && JSON.parse(c[1]?.body ?? "{}").method === "tools/call";
      } catch {
        return false;
      }
    });
    expect(toolsCall).toBeDefined();
    const body = JSON.parse(toolsCall![1].body);
    expect(body.params.arguments.repository_path).toBe("C:\\tmp\\mattermost-master");
  });

  it("outcome log lines carry the resolved repo for traceability", async () => {
    vi.stubGlobal(
      "fetch",
      stubDaemonFetch({
        content: [{ type: "text", text: "hi\n\n---\n\nContext:\ncode_context: x" }],
        structuredContent: { passthrough: false, total_tokens: 3, provenance: [] },
        isError: false,
      }),
    );
    const appLog = vi.fn().mockResolvedValue(true);
    const hooks = await v1Server({
      worktree: "/fallback",
      client: {
        app: { log: appLog },
        session: { get: async () => ({ location: { directory: "/session/repo" } }) },
      },
    });
    await hooks["chat.message"](
      { sessionID: "s9" },
      { message: { role: "user" }, parts: [{ type: "text", text: "hi" }] },
    );
    const messages = appLog.mock.calls.map((c: any[]) => (c[0] as any).body.message as string);
    expect(messages.some((m: string) => m.includes("repo=/session/repo"))).toBe(true);
  });
});

// requestOutputCompression describe block removed: the plugin no longer compresses —
// RTK (github.com/rtk-ai/rtk) owns the tool-output/command-rewrite layer.
