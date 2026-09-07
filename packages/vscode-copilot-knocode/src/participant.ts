import * as vscode from "vscode";
import { ensureDaemonReady, requestContextEnrichment } from "./daemon";

/**
 * Registers the `@knocode` chat participant.
 *
 * On every invocation (each user turn):
 *   1. resolves the active workspace root,
 *   2. fetches repository context from the Knocode daemon (`knocode_context`),
 *   3. assembles the prompt (history + [repository context] + user prompt),
 *   4. sends it to the user's own Copilot model via `request.model.sendRequest`,
 *   5. streams the response as markdown.
 *
 * This is the faithful analog of opencode's `chat.message` enrichment: inside
 * the participant, we own the prompt assembly, so context is injected on every
 * turn. Fail-open: if the daemon is down/mid-index it simply runs with the
 * bare prompt.
 */
export function registerKnocodeParticipant(context: vscode.ExtensionContext): void {
  const participant = vscode.chat.createChatParticipant(
    "chat.knocode",
    handler,
  );
  context.subscriptions.push(participant);
}

const handler: vscode.ChatRequestHandler = async (request, chatContext, stream, token) => {
  const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? process.cwd();
  stream.progress("Gathering repository context from the Knocode daemon…");

  // Bounded, fail-open readiness gate (cold-start indexing).
  await ensureDaemonReady();

  // Pre-model enrichment over the daemon MCP surface. One Date.now() pair — the
  // integration-boundary metric. request_id correlates with the daemon's log line.
  const startedAt = Date.now();
  const outcome = await requestContextEnrichment(
    request.prompt,
    cwd,
  );
  const latencyMs = Date.now() - startedAt;

  let contextText: string | undefined;
  // Pack metadata for the turn (0 on passthrough) — eval harnesses read these
  // from turn metadata instead of parsing log lines.
  let contextTokens = 0;
  let contextFiles = 0;
  if (outcome.kind === "enriched") {
    contextText = outcome.enrichedText;
    contextTokens = outcome.tokens;
    contextFiles = outcome.files;
    // The single integration-boundary metrics line: "Knocode added N ms to this
    // turn" — latency is client cost, tokens/files are pack size and breadth,
    // request_id joins with the daemon's "MCP knocode_context built" log line.
    console.log(`[knocode] context request_id=${outcome.requestId} latency=${latencyMs}ms tokens=${outcome.tokens} files=${outcome.files}`);
  } else {
    // Passthrough (no_context_hits / daemon_indexing / unreachable): run with the
    // bare prompt. The reason makes this line self-sufficient — no daemon log
    // access needed to classify it.
    console.log(`[knocode] context passthrough request_id=${outcome.requestId} reason=${outcome.reason} latency=${latencyMs}ms`);
  }

  const messages: vscode.LanguageModelChatMessage[] = [];

  // Conversation history (matches the chat-tutorial pattern).
  for (const turn of chatContext.history) {
    if (turn instanceof vscode.ChatResponseTurn) {
      const text = turn.response
        .filter((p) => p instanceof vscode.ChatResponseMarkdownPart)
        .map((p) => (p as vscode.ChatResponseMarkdownPart).value.value)
        .join("\n");
      if (text) messages.push(vscode.LanguageModelChatMessage.Assistant(text));
    } else if (turn instanceof vscode.ChatRequestTurn) {
      messages.push(vscode.LanguageModelChatMessage.User(turn.prompt));
    }
  }

  // Inject repository context, then the user's prompt.
  if (contextText) {
    messages.push(
      vscode.LanguageModelChatMessage.User(
        `[Repository context from Knocode]\n${contextText}`,
      ),
    );
  }
  messages.push(vscode.LanguageModelChatMessage.User(request.prompt));

  // Send to the user's own Copilot model and stream the reply.
  try {
    const response = await request.model.sendRequest(messages, {}, token);
    for await (const fragment of response.text) {
      stream.markdown(fragment);
    }
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    throw new Error(`Knocode model request failed: ${msg}`);
  }

  return {
    metadata: {
      knocodeContext: contextText ? "attached" : "none",
      knocodeRequestId: outcome.requestId,
      knocodeContextTokens: contextTokens,
      knocodeContextFiles: contextFiles,
      // Client-side enrichment cost in ms (the requestContextEnrichment call only;
      // readiness-gate waiting is excluded). Recorded on passthrough turns too, so
      // evals can measure what a failed/unanswered request cost.
      knocodeLatencyMs: latencyMs,
    },
  };
};