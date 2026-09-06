# Knocode for GitHub Copilot (VS Code extension)

A VS Code extension that adds an **`@knocode` chat participant** to GitHub Copilot
Chat. On every invocation it enriches your prompt with repository context from the
local [Knocode](https://github.com/LeonOrtega/knocode) daemon, then hands the
enriched conversation to **your own Copilot model** (`request.model.sendRequest`) and
streams the reply — no separate LLM, no extra credits.

This is the true per-turn analog of opencode's `chat.message` hook: inside the
participant we own the prompt assembly, so context is injected on **every** prompt.
Use it alongside the
[`knocode-copilot` Agent Plugin](../knocode-copilot-plugin), which adds automatic
(no-`@`) context injection via agent hooks.

## What it gives you

- **Per-turn repository context** — `@knocode <your prompt>` fetches context from
  the daemon (`knocode_context`) and prepends it before the model sees the prompt.
- **Uses your own model** — calls `request.model.sendRequest`, so it consumes your
  normal Copilot model (no self-hosted LLM, unlike a GitHub App extension).
- **Conversation history** — prior turns are included so the reply stays on track.
- **Fail-open** — if the daemon is down, mid-index, or returns no hits, it simply
  runs with your bare prompt.

## Requirements

- VS Code **1.98+** with GitHub Copilot Chat.
- The Knocode daemon running: `knocode init` once per repo, then `knocode serve`.

## Install / package

```bash
cd packages/vscode-copilot-knocode
npm install
npm run build          # tsc -> dist/
npm run typecheck      # optional
npm test               # vitest for the daemon client
npm run package        # vsce -> .vsix  (requires @vscode/vsce)
```

Install the generated `.vsix` via the Extensions view (**Install from VSIX…**), then
in Copilot Chat type `@knocode` followed by your prompt.

## Development

The daemon client in `src/daemon.ts` reuses the exact wire contract of the OpenCode
plugin (MCP over `POST /mcp`, `GET /health` readiness gate). Any change to the daemon
tool surface (`knocode_context`) is picked up automatically — the
extension never reimplements retrieval.

## Configuration

The daemon URL and timeouts come from the same env vars as the other integrations:

| Variable | Default |
|----------|---------|
| `KNOCODE_DAEMON_URL` | `http://127.0.0.1:9527` |
| `KNOCODE_TIMEOUT_MS` | `30000` |

## License

MIT