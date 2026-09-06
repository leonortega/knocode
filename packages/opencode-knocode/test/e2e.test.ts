import { describe, it, expect, vi } from "vitest";
import { mcpCall, requestContextEnrichment } from "../src/index";

/**
 * Canonical E2E: OpenCode (session.prompt hook) → Knocode MCP surface
 * (`POST /mcp` → `tools/call knocode_context`) → BuildContext → enriched text.
 * Uses mocked fetch to simulate a daemon doing deterministic retrieval only
 * (no Router/LiteLLM — see REMOVED_TOOLS.md).
 *
 * The legacy `POST /hook` MessageRewrite fallback was removed from the plugin:
 * MCP is the only enrichment path.
 */

const mcpOk = (result: any) =>
  Promise.resolve({
    ok: true,
    status: 200,
    json: async () => ({ jsonrpc: "2.0", id: 1, result }),
  } as any);

describe("E2E: OpenCode → Knocode MCP knocode_context → context pack", () => {
  it("returns the enriched text with provenance from the context pack", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpOk({
        content: [
          {
            type: "text",
            text: "implement auth\n\n---\n\nContext:\ncode_context: // src/auth.rs:10 fn authenticate()",
          },
        ],
        structuredContent: {
          type: "context",
          passthrough: false,
          total_tokens: 8500,
          provenance: [{ path: "src/auth.rs", source: "code", retriever: "tantivy", score: 0.92 }],
          repository_state: "deadbeef12345678",
        },
        isError: false,
      }),
    );

    const result = await requestContextEnrichment("implement auth", "/repo/eshop", {
      url: "http://127.0.0.1:9527",
      timeoutMs: 1000,
      fetchImpl: mockFetch as any,
    });

    expect(result?.enrichedText).toContain("Context:");
    expect(result?.enrichedText).toContain("src/auth.rs:10");
    expect(result?.tokens).toBe(8500);
    expect(result?.files).toBe(1);

    const [, init] = mockFetch.mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.jsonrpc).toBe("2.0");
    expect(body.method).toBe("tools/call");
    expect(body.params.name).toBe("knocode_context");
    expect(body.params.arguments.prompt).toBe("implement auth");
    // TASK-036/F-7: the agent workspace root travels with every enrichment call
    expect(body.params.arguments.repository_path).toBe("/repo/eshop");
  });

  it("fail-open (null) when daemon unreachable — never breaks admission", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const enriched = await requestContextEnrichment("implement auth", "/repo", {
      fetchImpl: mockFetch as any,
    });
    expect(enriched).toBeNull(); // caller admits the prompt untouched
    errSpy.mockRestore();
  });

  it("fail-open (null) while daemon is indexing (-32001)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    } as any);
    const enriched = await requestContextEnrichment("hi", "/repo", { fetchImpl: mockFetch as any });
    expect(enriched).toBeNull(); // prompt admitted untouched, retry next message
  });

  it("fail-open (null) on zero context hits (passthrough contract, TASK-031/F-2)", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpOk({
        content: [{ type: "text", text: "zzzqqq unrelated" }],
        structuredContent: { type: "context", passthrough: true, reason: "no_context_hits" },
        isError: false,
      }),
    );
    const enriched = await requestContextEnrichment("zzzqqq unrelated", "/repo", {
      fetchImpl: mockFetch as any,
    });
    // The plugin must NOT rewrite a zero-hit prompt — user text stays byte-identical
    expect(enriched).toBeNull();
  });
});

describe("MCP surface contract (used by the session.prompt hook)", () => {
  it("tools/list reports knocode_context as the single tool", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpOk({ tools: [{ name: "knocode_context", description: "Repository context for a prompt" }] }),
    );
    const out = await mcpCall("tools/list", {}, { url: "http://127.0.0.1:9527", fetchImpl: mockFetch as any });
    expect(out.kind).toBe("ok");
    if (out.kind === "ok") {
      expect(out.result.tools).toHaveLength(1);
      expect(out.result.tools[0].name).toBe("knocode_context");
    }
    const [, init] = mockFetch.mock.calls[0];
    expect(JSON.parse(init.body).method).toBe("tools/list");
  });

  it("sends a well-formed JSON-RPC 2.0 envelope with incrementing ids", async () => {
    const mockFetch = vi.fn().mockImplementation(async () => mcpOk({}));
    await mcpCall("ping", {}, { url: "http://127.0.0.1:9527", fetchImpl: mockFetch as any });
    await mcpCall("ping", {}, { url: "http://127.0.0.1:9527", fetchImpl: mockFetch as any });
    const first = JSON.parse(mockFetch.mock.calls[0][1].body);
    const second = JSON.parse(mockFetch.mock.calls[1][1].body);
    expect(first.jsonrpc).toBe("2.0");
    expect(second.id).toBeGreaterThan(first.id);
  });
});
