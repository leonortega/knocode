#!/usr/bin/env node
/**
 * Version single-source-of-truth sync (repo-wide).
 *
 * Root `Cargo.toml` [workspace.package].version is THE version of knocode.
 * Everything in MIRRORS below is *derived* — edit Cargo.toml, then run:
 *
 *   node scripts/sync-version.mjs          # apply
 *   node scripts/sync-version.mjs --check  # verify only (CI / release gate)
 *
 * Deliberately NOT touched:
 *   - CHANGELOG.md and docs/*          (historical facts — v0.9.11 is a
 *                                       statement about the past, not a mirror)
 *   - knocode.json, winget/**          (release-pipeline outputs — written by
 *                                       the release workflow with real SHA256s)
 *   - Cargo.lock                       (cargo owns it; `cargo check` refreshes)
 *
 * Mirrors updated here:
 *   - packages/<pkg>/package.json          (5 npm packages)
 *   - packages/knocode-copilot-plugin/plugin.json
 *   - release.toml                         ([workspace] version for cargo-release)
 *   - docs/ROADMAP.md "## Current Version: vX.Y.Z" heading
 */

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const CHECK_ONLY = process.argv.includes("--check");

// ── Read the source of truth ──────────────────────────────────────────────
const cargo = readFileSync(path.join(root, "Cargo.toml"), "utf8");
const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version || !/^\d+\.\d+\.\d+/.test(version)) {
  console.error(`[sync-version] FATAL: could not read a valid workspace.package version from Cargo.toml`);
  process.exit(1);
}

let failures = 0;
let updated = 0;
const fail = (file, msg) => { console.error(`[sync-version] DRIFT  ${file}: ${msg}`); failures++; };
const note = (file, msg) => { console.log(`[sync-version] ${CHECK_ONLY ? "ok    " : "set   "} ${file}: ${msg}`); updated++; };

// ── JSON files: version field ─────────────────────────────────────────────
const jsonFiles = [
  "packages/knocode-client/package.json",
  "packages/knocode-mcp/package.json",
  "packages/opencode-knocode/package.json",
  "packages/vscode-copilot-knocode/package.json",
  "packages/knocode-copilot-plugin/plugin.json",
];
for (const rel of jsonFiles) {
  const p = path.join(root, rel);
  let raw;
  try {
    raw = readFileSync(p, "utf8");
  } catch (e) {
    fail(rel, `unreadable (${e.message})`);
    continue;
  }
  // Surgical text replacement (NOT parse+stringify): reformatting would rewrite
  // unrelated lines (array expansion, CRLF→LF) and pollute the git diff.
  // Top-level "version" key = first "version" at the start of a line.
  const re = /^(\s*"version"\s*:\s*")([^"]+)(")/m;
  const m = raw.match(re);
  if (!m) {
    fail(rel, `no top-level "version" field found`);
    continue;
  }
  const oldVersion = m[2];
  if (oldVersion === version) continue;
  if (CHECK_ONLY) { fail(rel, `version ${oldVersion} != ${version}`); continue; }
  writeFileSync(p, raw.replace(re, `$1${version}$3`));
  note(rel, `${oldVersion} → ${version}`);
}

// ── release.toml: [workspace] version ─────────────────────────────────────
{
  const rel = "release.toml";
  const p = path.join(root, rel);
  const raw = readFileSync(p, "utf8");
  const m = raw.match(/^\s*version\s*=\s*"([^"]*)"/m);
  if (!m) {
    fail(rel, "no [workspace] version line found");
  } else if (m[1] !== version) {
    if (CHECK_ONLY) {
      fail(rel, `version ${m[1]} != ${version}`);
    } else {
      const next = raw.replace(
        /^(\s*version\s*=\s*")[^"]*(")/m,
        `$1${version}$2`,
      );
      if (next === raw) fail(rel, "replacement produced no change");
      else { writeFileSync(p, next); note(rel, `${m[1]} → ${version}`); }
    }
  }
}

// ── docs/ROADMAP.md "## Current Version" heading ──────────────────────────
{
  const rel = "docs/ROADMAP.md";
  const p = path.join(root, rel);
  let raw;
  try {
    raw = readFileSync(p, "utf8");
  } catch {
    raw = null; // optional — skip silently
  }
  if (raw !== null) {
    const heading = /^## Current Version:\s*(\S+)\s*$/m;
    const m = raw.match(heading);
    if (!m) {
      // Heading absent: nothing to keep in sync (allowed)
    } else if (m[1] !== `v${version}`) {
      if (CHECK_ONLY) {
        fail(rel, `heading says ${m[1]}, Cargo.toml says v${version}`);
      } else {
        writeFileSync(p, raw.replace(heading, `## Current Version: v${version}`));
        note(rel, `${m[1]} → v${version}`);
      }
    }
  }
}

// ── Summary ───────────────────────────────────────────────────────────────
if (failures > 0) {
  console.error(
    `\n[sync-version] ${failures} drift issue(s) found. ` +
      (CHECK_ONLY ? "Run `node scripts/sync-version.mjs` to fix." : ""),
  );
  process.exit(1);
}
console.log(
  CHECK_ONLY
    ? `[sync-version] all mirrors match workspace version ${version}`
    : `[sync-version] done — ${updated} file(s) aligned to workspace version ${version}`,
);
