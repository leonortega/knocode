# knocode-copilot

Knocode AI Runtime plugin for **GitHub Copilot (VS Code)** — a self-contained
[Agent Plugin](https://code.visualstudio.com/docs/agent-customization/agent-plugins)
that enriches agent sessions with repository context and surfaces the Knocode tools,
all backed by the local Knocode daemon (`knocode serve` on `http://127.0.0.1:9527`).

This is the Copilot analog of [`packages/opencode-knocode`](../opencode-knocode). It
reuses the exact same daemon surface — it never reimplements retrieval or compression.
(Context is injected per prompt via `UserPromptSubmit`; the companion **`@knocode` chat
participant** extension at [`packages/vscode-copilot-knocode`](../vscode-copilot-knocode)
adds an on-demand `@knocode` command on top.)

## What it does

Because VS Code's agent hooks can inject context but cannot rewrite the user's prompt,
this plugin maps the context behavior onto the Copilot hook surface:

| Hook | OpenCode analog | What Knocode does |
|------|-----------------|-------------------|
| `SessionStart` | prompt enrichment (warm) | injects a repository-context digest via `knocode_context` |
| `UserPromptSubmit` | `session.prompt` (per-prompt) | injects context retrieved from the user's actual prompt via `knocode_context` |

> **Tool-output compression?** That is [RTK](https://github.com/rtk-ai/rtk)'s job now —
> the knocode installer can install RTK and wire its own Copilot integration for you.

Plus a bundled MCP server (`servers/knocode-mcp.mjs`) that exposes
`knocode_context` to Copilot's agent mode, so the model can call it directly on demand.

**Fail-open everywhere:** if the daemon is unreachable, mid-index, or errors, every
hook returns `{}` and exits `0` — Copilot proceeds untouched. Hooks never stall or
break the agent.

## Layout

```
knocode-copilot/
├── plugin.json                  # Agent Plugins 1.0 manifest
├── mcp.json                     # portable MCP config -> bundled stdio server
├── servers/
│   └── knocode-mcp.mjs          # GENERATED from packages/knocode-mcp (edits ignored)
├── com.github.copilot/
│   └── hooks/hooks.json         # SessionStart / UserPromptSubmit wiring
└── scripts/
    ├── knocode-hook.mjs         # cross-platform hook handler (fail-open)
    └── build.mjs                # regenerates servers/knocode-mcp.mjs
```

## Requirements

- VS Code with **agent plugins enabled** — set `chat.plugins.enabled` to `true`.
- GitHub Copilot (Chat) installed.
- **Node.js ≥ 18** on `PATH` (hooks + MCP server launch `node`).
- The Knocode daemon **running**: `knocode init` once per repo, then `knocode serve`.

## Install

Install from the VS Code **Extensions** view (`@agentPlugins @recommended`) if published
to a marketplace, or clone/copy the plugin directory into the local agentPlugins cache
and specify it in settings:

```jsonc
// settings.json (workspace)
{
  "chat.plugins.enabled": true,
  "enabledPlugins": {
    "knocode-copilot": true
  }
}
```

Plugin output can be inspected in **Output → GitHub Copilot Chat Hooks**.

## Configuration (environment variables)

The hook handler and MCP server read the same env vars as the OpenCode plugin:

| Variable | Default | Used by |
|----------|---------|---------|
| `KNOCODE_DAEMON_URL` | `http://127.0.0.1:9527` | hooks + MCP server |
| `KNOCODE_TIMEOUT_MS` | `15000` | per MCP call timeout |
| `KNOCODE_READY_TIMEOUT_MS` | `5000` (`0` disables) | `SessionStart` readiness wait |

## Development

`servers/knocode-mcp.mjs` is a rendered copy of `packages/knocode-mcp` — the daemon
owns the tool surface so it can never drift. To regenerate after changing that package:

```bash
cd packages/knocode-mcp && npm install && npm run build
cd ../knocode-copilot-plugin && node scripts/build.mjs
```

`scripts/knocode-hook.mjs` is dependency-free and validated with `node --check`.

## Security

Hooks run shell commands with the same permissions as VS Code and the hook input is
placed on stdin — treat the plugin as trusted. The Knocode daemon binds to loopback
and never sends credentials; the hook only forwards repository text and tool output
to the local daemon.

## License

MIT