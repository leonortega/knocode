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
 * This script ALSO checks the daemon wire-contract delimiter (see checkWireContract
 * below): the context answer's prompt/context delimiter is hardcoded in two consumers
 * that never touch the vendored client, so they are verified against the daemon's
 * `format!` template in crates/knocode-daemon/src/http_server.rs. That check runs in
 * BOTH modes (a mismatch is not fixable by re-vendoring — it fails the build loudly).
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
// EOL normalization is REQUIRED, not cosmetic: this script is committed LF-only,
// while client.ts / vendored copies were committed CRLF. Byte comparison then
// flips verdict per platform depending on the runner's git EOL filtering (Windows
// raw blobs vs Linux checkout normalization) — drift reports on one OS, "up to
// date" on another, for identical content. The contract is therefore defined in
// LF: compare and write normalized.
const toLF = (s) => s.replace(/\r\n/g, "\n");

for (const target of targets) {
  // stamp included: on autocrlf=true checkouts the script file itself materializes
  // CRLF, which would inject \r into the stamp literal — normalize everything.
  const rendered = toLF(stamp + source);
  if (check) {
    let existing = null;
    try {
      existing = readFileSync(target, "utf8");
    } catch {
      /* missing */
    }
    if (existing === null || toLF(existing) !== rendered) {
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
  if (existing !== null && toLF(existing) === rendered) {
    console.log(`[vendor-client] up to date: ${path.relative(repoRoot, target)}`);
    continue;
  }
  writeFileSync(target, rendered);
  console.log(`[vendor-client] vendored: ${path.relative(repoRoot, target)}`);
}

// ── Wire-contract drift check ────────────────────────────────────────────────
// The daemon's `knocode_context` answer is a FULL replacement whose prefix is the
// original prompt, delimited by the `format!` template in http_server.rs
// (`format!("{}\n\n---\n\nContext:\n{}", message, yaml)`). Three things must agree
// with that template:
//   - packages/opencode-knocode/src/index.ts CONTEXT_DELIMITER (idempotency guard,
//     V1 chat.message + V2 session.prompt hooks)
//   - packages/knocode-copilot-plugin/scripts/knocode-hook.mjs CONTEXT_DELIMITER
//     (prefix-stripping so UserPromptSubmit doesn't duplicate the user's prompt)
// The vendored client passes `enrichedText` through untouched and never sees the
// delimiter, so the byte-copy check above CANNOT catch this drift — hence this
// dedicated check. It runs in BOTH modes: re-vendoring can't fix a mismatch, so a
// divergence fails the build loudly instead of shipping a broken guard.

const daemonSourcePath = path.join(repoRoot, "crates", "knocode-daemon", "src", "http_server.rs");

const delimiterConsumers = [
  {
    label: "packages/opencode-knocode/src/index.ts",
    file: path.join(repoRoot, "packages", "opencode-knocode", "src", "index.ts"),
    pattern: /export const CONTEXT_DELIMITER = ("(?:[^"\\]|\\.)*")/,
  },
  {
    label: "packages/knocode-copilot-plugin/scripts/knocode-hook.mjs",
    file: path.join(repoRoot, "packages", "knocode-copilot-plugin", "scripts", "knocode-hook.mjs"),
    pattern: /const CONTEXT_DELIMITER = ("(?:[^"\\]|\\.)*")/,
  },
];

/**
 * @returns {boolean} true when every consumer delimiter matches the daemon template.
 */
function checkWireContract() {
  let daemonSrc;
  try {
    daemonSrc = readFileSync(daemonSourcePath, "utf8");
  } catch (err) {
    console.error(`[wire-contract] cannot read daemon source: ${path.relative(repoRoot, daemonSourcePath)}: ${err?.message ?? err}`);
    return false;
  }

  // The daemon assembles the replacement as format!("<template>", message, yaml).
  // Capture the Rust string literal (Rust escapes \n \t \r \" \\ coincide with JSON
  // escapes, so JSON.parse decodes it), then strip the format! pieces: {{ }} are
  // literal braces and {…} ({}, {0}, {name}) are placeholders for the two args —
  // what remains is exactly the text between/around the substituted prompt and YAML,
  // i.e. the delimiter consumers must hardcode.
  const m = daemonSrc.match(/format!\(\s*"((?:[^"\\]|\\.)*)",\s*message,\s*yaml/);
  if (!m) {
    console.error(`[wire-contract] DRIFT: no format!("…", message, yaml) template found in ${path.relative(repoRoot, daemonSourcePath)} — the context answer layout changed. Update CONTEXT_DELIMITER in the consumers and this check's regex to the new format.`);
    return false;
  }
  let expected;
  try {
    expected = JSON.parse(`"${m[1]}"`)
      .replace(/\{\{/g, "{")
      .replace(/\}\}/g, "}")
      .replace(/\{[^{}]*\}/g, "");
  } catch {
    console.error(`[wire-contract] cannot decode the daemon format! literal "${m[1]}"`);
    return false;
  }

  let ok = true;
  for (const { label, file, pattern } of delimiterConsumers) {
    let src;
    try {
      src = readFileSync(file, "utf8");
    } catch (err) {
      console.error(`[wire-contract] cannot read ${label}: ${err?.message ?? err}`);
      ok = false;
      continue;
    }
    const dm = src.match(pattern);
    if (!dm) {
      console.error(`[wire-contract] DRIFT: ${label} no longer declares CONTEXT_DELIMITER — restore the guard or update scripts/vendor-client.mjs`);
      ok = false;
      continue;
    }
    let actual;
    try {
      actual = JSON.parse(dm[1]);
    } catch {
      console.error(`[wire-contract] cannot decode CONTEXT_DELIMITER in ${label}`);
      ok = false;
      continue;
    }
    if (actual !== expected) {
      console.error(`[wire-contract] DRIFT: ${label} CONTEXT_DELIMITER ${JSON.stringify(actual)} != daemon template ${JSON.stringify(expected)} — sync it with http_server.rs`);
      ok = false;
    } else {
      console.log(`[wire-contract] ok: ${label} CONTEXT_DELIMITER matches http_server.rs`);
    }
  }
  return ok;
}

// Runs in both modes and even when vendoring already failed, so one CI run reports
// every drift; the exit code combines both checks.
const contractOk = checkWireContract();
process.exit(failed || !contractOk ? 1 : 0);
