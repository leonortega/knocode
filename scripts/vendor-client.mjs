/**
 * Vendor the shared daemon client into the agent plugins.
 *
 * packages/knocode-client/src/client.ts is the SINGLE SOURCE OF TRUTH for the
 * daemon wire contract (MCP POST /mcp, /health readiness, request_id correlation,
 * tagged context outcomes). Each consumer gets a SOURCE copy at build time so its
 * dist stays self-contained:
 *
 *   - packages/opencode-knocode        → src/vendor/knocode-client.ts (ESM)
 *   - packages/vscode-copilot-knocode  → src/vendor/knocode-client.ts (CJS)
 *
 * Runs automatically via the consumers' build/test/typecheck npm scripts (chained
 * explicitly, NOT npm pre-hooks — `ignore-scripts` configs skip those). `--check`
 * (used by CI, release.yml) exits 1 if any committed copy is missing or drifted.
 *
 * Why source-vendoring and not a runtime dependency: the release zip ships each
 * plugin's dist WITHOUT node_modules (README "no npm registry needed"), and the two
 * consumers use different module systems (ESM vs CJS). Vendoring keeps every artifact
 * self-contained — the same pattern knocode-copilot-plugin already uses to bundle
 * knocode-mcp's dist (scripts/build.mjs).
 */
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = path.join(repoRoot, "packages", "knocode-client", "src", "client.ts");

const targets = [
  path.join(repoRoot, "packages", "opencode-knocode", "src", "vendor", "knocode-client.ts"),
  path.join(repoRoot, "packages", "vscode-copilot-knocode", "src", "vendor", "knocode-client.ts"),
];

const check = process.argv.includes("--check");

let source;
try {
  source = readFileSync(sourcePath, "utf8");
} catch (err) {
  console.error(`[vendor-client] cannot read source of truth: ${sourcePath}: ${err?.message ?? err}`);
  process.exit(1);
}

const stamp = `/**
 * VENDORED COPY — DO NOT EDIT.
 * Source of truth: packages/knocode-client/src/client.ts
 * Regenerate with \`npm run vendor\` in this package (or edit the source file).
 * Drift from the source fails CI (scripts/vendor-client.mjs --check).
 */
`;

let failed = false;
for (const target of targets) {
  const rendered = stamp + source;
  if (check) {
    let existing = null;
    try {
      existing = readFileSync(target, "utf8");
    } catch {
      /* missing */
    }
    if (existing !== rendered) {
      console.error(`[vendor-client] DRIFT: ${path.relative(repoRoot, target)} differs from packages/knocode-client/src/client.ts — run \`npm run vendor\``);
      failed = true;
    } else {
      console.log(`[vendor-client] ok (check): ${path.relative(repoRoot, target)}`);
    }
    continue;
  }
  mkdirSync(path.dirname(target), { recursive: true });
  const existing = (() => {
    try {
      return readFileSync(target, "utf8");
    } catch {
      return null;
    }
  })();
  if (existing === rendered) {
    console.log(`[vendor-client] up to date: ${path.relative(repoRoot, target)}`);
    continue;
  }
  writeFileSync(target, rendered);
  console.log(`[vendor-client] vendored: ${path.relative(repoRoot, target)}`);
}

process.exit(failed ? 1 : 0);
