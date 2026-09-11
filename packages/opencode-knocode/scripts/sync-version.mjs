/**
 * Version single-source-of-truth bridge.
 *
 * Root `Cargo.toml` [workspace.package].version is THE version of knocode.
 * This script makes package.json / package-lock.json CONSUME it: it runs
 * automatically on `npm run build` (prebuild) and `prepack`, so the manifests
 * can never drift from the workspace version — no manual syncing, ever.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const pkgDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.resolve(pkgDir, "..", "..");
const cargoPath = path.join(root, "Cargo.toml");

const cargo = readFileSync(cargoPath, "utf8");
const match = cargo.match(/^version\s*=\s*"([^"]+)"/m);
if (!match) {
  console.error(`[sync-version] no workspace version found in ${cargoPath}`);
  process.exit(1);
}
const version = match[1];

let changed = false;
for (const file of ["package.json", "package-lock.json"]) {
  const p = path.join(pkgDir, file);
  let raw;
  try {
    raw = readFileSync(p, "utf8");
  } catch {
    continue; // optional file
  }
  const json = JSON.parse(raw);
  if (json.version !== version) {
    json.version = version;
    changed = true;
  }
  // lockfileVersion >= 2 stores the root package again under packages[""]
  if (file === "package-lock.json" && json.packages?.[""] && json.packages[""]?.version !== version) {
    json.packages[""].version = version;
    changed = true;
  }
  writeFileSync(p, JSON.stringify(json, null, 2) + "\n");
}

console.log(
  changed
    ? `[sync-version] manifests aligned to workspace version ${version}`
    : `[sync-version] already at workspace version ${version}`,
);
