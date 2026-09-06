# opencode-knocode

Knocode AI Runtime plugin for [OpenCode](https://opencode.ai), built on the **V2 plugin spec** (`Plugin.define` + `ctx.session.hook("prompt")`).

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
ctx.session.hook("prompt")          ← V2 admission hook (owned, mutable draft)
     │
     ▼
MCP POST /mcp → tools/call knocode_context
     │
     ▼
event.prompt.text = enriched        ← becomes the canonical persisted user input
```

## Behavior notes

- **Fail-open** — daemon unreachable, indexing, or zero context hits leaves the prompt byte-identical.
- **Idempotent** — a `<knocode_context>` marker guard prevents double-enrichment if a hook replay re-runs on an already-enriched draft (prompt hooks are not an exactly-once boundary per the V2 docs).
- **Attachment mentions** — file-mention offsets are cleared only when the rewrite breaks the original-text prefix (the daemon preserves the prefix, so mentions normally survive).
- **Beta API** — targets `@opencode-ai/plugin` `beta` (OpenCode V2). The V2 plugin API can change before stable release; pin versions accordingly.

Tool-output compression is intentionally not part of this plugin — [RTK](https://github.com/rtk-ai/rtk) owns that layer and ships its own OpenCode plugin.

Fail-open: daemon unreachable or non-2xx returns no-op.

## License

MIT
