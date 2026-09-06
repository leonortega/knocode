#!/usr/bin/env node
/**
 * Renders `packages/knocode-mcp/dist/index.js` (a single-file, self-contained
 * stdio -> HTTP MCP proxy with zero runtime dependencies) into this plugin's
 * `servers/knocode-mcp.mjs`, so the Agent Plugin bundle is portable and can be
 * dropped into VS Code's agentPlugins directory without a separate install step.
 *
 * The daemon owns the canonical MCP tools (`knocode_context`; compression lives in
 * RTK, github.com/rtk-ai/rtk); this plugin never reimplements them — the copy is
 * byte-identical to the npm package, so the surface can never drift from
 * `packages/knocode-mcp`.
 *
 * Usage:   node scripts/build.mjs
 * Required pre-step (once):  cd packages/knocode-mcp && npm install && npm run build
 */

import { copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, "../../knocode-mcp/dist/index.js");
const dest = resolve(here, "../servers/knocode-mcp.mjs");

try {
  readFileSync(src); // throws if missing -> instruct to build knocode-mcp first
} catch {
  console.error(
    `[knocode-copilot-plugin] Missing ${src}. Build it first:\n` +
      `  cd packages/knocode-mcp && npm install && npm run build`,
  );
  process.exit(1);
}

mkdirSync(dirname(dest), { recursive: true });
copyFileSync(src, dest);
console.log(`[knocode-copilot-plugin] bundled MCP server -> ${dest}`);