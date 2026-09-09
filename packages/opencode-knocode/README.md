# opencode-knocode

Knocode AI Runtime plugin for [OpenCode](https://opencode.ai), supporting **both the V1 and V2 plugin APIs** from one entrypoint:

- **OpenCode 1.x (V1)** — default export carries a `server()` function returning a `chat.message` hook (message enrichment in place).
- **OpenCode 2 beta (V2)** — default export carries `Plugin.define({ id, setup })` with the `ctx.session.hook("prompt")` admission hook (owned, mutable draft).

V1 reads the default export's `server()`; V2 reads its `id` + `setup()` and ignores `server()` — the documented "Support V1" pattern.

Enriches prompts with repository context via the Knocode daemon (`knocode serve` on `http://127.0.0.1:9527`), during prompt admission — before attachment and skill resolution. The enriched text becomes the canonical persisted user input in the session.

> **Tool-output compression / command rewriting?** That is [RTK](https://github.com/rtk-ai/rtk)'s job now — the knocode installer can install and wire RTK's own integrations for you.

## Install

```bash
npm install opencode-knocode
# or publish and add to opencode config:
```

`opencode.json` / `opencode.jsonc`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "plugins": ["opencode-knocode"]
}
```

OpenCode installs npm plugins automatically via `bun` to `~/.cache/opencode/node_modules/` on startup. Alternatively keep a local copy in `.opencode/plugins/`.

## Configuration

| Env var | Default | Description |
|---------|---------|-------------|
| `KNOCODE_DAEMON_URL` | `http://127.0.0.1:9527` | Daemon base URL |
| `KNOCODE_TIMEOUT_MS` | `30000` | HTTP timeout |

## Scripts

| Script | Description |
|--------|-------------|
| `npm run build` | Compile TypeScript to `dist/` (compiler) |
| `npm run typecheck` | Type-check without emit |
| `npm run test` | Run unit tests (vitest) |
| `npm run test:watch` | Watch mode |
| `npm run dev` | Watch compiler |

## Development

```bash
npm install
npm run build
npm test
```

## How it works

```text
User prompt
     │
     ▼
V1: server() → "chat.message"        V2: ctx.session.hook("prompt")  ← admission hook
     │                                     │
     └──────────────┬──────────────────────┘
                    ▼
MCP POST /mcp → tools/call knocode_context
                    │
                    ▼
enriched text                 ← V2: canonical persisted input · V1: message.content mutated in place
```

## Behavior notes

- **V1 + V2** — one entrypoint serves OpenCode 1.x (`chat.message`) and the 2.0 beta (`session.prompt`); no config change needed when switching OpenCode versions.
- **Fail-open** — daemon unreachable, indexing, or zero context hits leaves the prompt byte-identical.
- **Idempotent** — a delimiter guard (`\n\n---\n\nContext:\n`, the daemon's real wire format) prevents double-enrichment on hook replays (prompt hooks are not an exactly-once boundary per the V2 docs).
- **Attachment mentions (V2)** — file-mention offsets are cleared only when the rewrite breaks the original-text prefix (the daemon preserves the prefix, so mentions normally survive).
- **Beta API (V2)** — targets `@opencode-ai/plugin` `beta` (OpenCode V2). The V2 plugin API can change before stable release; pin versions accordingly.

Tool-output compression is intentionally not part of this plugin — [RTK](https://github.com/rtk-ai/rtk) owns that layer and ships its own OpenCode plugin.

Fail-open: daemon unreachable or non-2xx returns no-op.

## License

MIT
