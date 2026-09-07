import { describe, it, expect, vi } from "vitest";
import { requestContextEnrichment } from "../src/index";

/**
 * Canonical E2E: OpenCode (session.prompt hook) → Knocode MCP surface
 * (`POST /mcp` → `tools/call knocode_context`) → BuildContext → enriched text.
 * Uses mocked fetch to simulate a daemon doing deterministic retrieval only
 * (no Router/LiteLLM — see REMOVED_TOOLS.md).
 *
 * The legacy `POST /hook` MessageRewrite fallback was removed from the plugin:
 * MCP is the only enrichment path.
 *
 * Client contract tests (JSON-RPC envelope, readiness gate, outcome taxonomy)
 * live in packages/knocode-client/test — the single source of truth this plugin
 * vendors at build time.
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

    expect(result.kind).toBe("enriched");
    expect(result.enrichedText).toContain("Context:");
    expect(result.enrichedText).toContain("src/auth.rs:10");
    expect(result.tokens).toBe(8500);
    expect(result.files).toBe(1);

    const [, init] = mockFetch.mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.jsonrpc).toBe("2.0");
    expect(body.method).toBe("tools/call");
    expect(body.params.name).toBe("knocode_context");
    expect(body.params.arguments.prompt).toBe("implement auth");
    // TASK-036/F-7: the agent workspace root travels with every enrichment call
    expect(body.params.arguments.repository_path).toBe("/repo/eshop");
    // §7 request correlation: client-generated request_id travels with every call
    expect(body.params.arguments.request_id).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("fail-open (tagged passthrough) when daemon unreachable — never breaks admission", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const mockFetch = vi.fn().mockRejectedValue(new Error("ECONNREFUSED"));
    const outcome = await requestContextEnrichment("implement auth", "/repo", {
      fetchImpl: mockFetch as any,
    });
    // caller admits the prompt untouched; the reason classifies without daemon logs
    expect(outcome.kind).toBe("passthrough");
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("daemon_unreachable");
    errSpy.mockRestore();
  });

  it("fail-open (tagged passthrough) while daemon is indexing (-32001)", async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ jsonrpc: "2.0", id: 1, error: { code: -32001, message: "daemon_indexing" } }),
    } as any);
    const outcome = await requestContextEnrichment("hi", "/repo", { fetchImpl: mockFetch as any });
    expect(outcome.kind).toBe("passthrough"); // prompt admitted untouched, retry next message
    if (outcome.kind === "passthrough") expect(outcome.reason).toBe("mcp_error_-32001");
  });

  it("fail-open (tagged passthrough) on zero context hits (passthrough contract, TASK-031/F-2)", async () => {
    const mockFetch = vi.fn().mockImplementation(async () =>
      mcpOk({
        content: [{ type: "text", text: "zzzqqq unrelated" }],
        structuredContent: { type: "context", passthrough: true, reason: "no_context_hits" },
        isError: false,
      }),
    );
    const outcome = await requestContextEnrichment("zzzqqq unrelated", "/repo", {
      fetchImpl: mockFetch as any,
    });
    // The plugin must NOT rewrite a zero-hit prompt — user text stays byte-identical
    expect(outcome).toEqual({
      kind: "passthrough",
      reason: "no_context_hits",
      requestId: expect.any(String),
    });
  });
});
