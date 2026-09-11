---
name: knocode
description: Use the local Knocode AI Runtime via its knocode_context MCP tool for ranked, cross-file repository context before answering code questions; run knocode init/doctor via the installed binary when the index is missing.
license: MIT
metadata:
  audience: developers
  workflow: knocode-init
---

# Knocode

**Local AI Runtime** — ranked, cross-file repository context over MCP. Call it before answering code questions; the index is per-repository and daemon-backed.

## When to use

- **Code questions** (where is X, how does Y work, implement Z, fix this bug): call `knocode_context` FIRST with the user's question, then answer using the returned context (file paths, snippets, provenance scores).
- **Not for:** single literal `grep -n "exact string"` where `rg` is faster — use `rg` directly.

## Calling `knocode_context` (MCP)

- **Tool:** `knocode_context` on the `knocode` MCP server (wired globally by the installer).
- **Arguments:** `prompt` = the user's actual question (verbatim), `repository_path` = the workspace root absolute path.
- **Response shape:** the answer is a FULL replacement — `<original prompt>\n\n---\n\nContext:\n<yaml>`. The prefix is the question you already have: **use only the context block**, never echo the prompt back.
- **Empty/passthrough answer:** no context hits (or daemon indexing) — proceed with normal tools (`read`, `grep`, `rg`); do not retry in a loop.

## Binary location — never search

**Installed absolute (always):**

- Windows: `%USERPROFILE%\.knocode\bin\knocode.exe`
- Unix: `~/.knocode/bin/knocode`

Do **not** walk the filesystem looking for the binary (`find`, recursive `ls`, `where knocode` may resolve the dev checkout instead). If bare `knocode` is not on PATH, use the absolute path or restart the shell.

## CLI commands (run from the project root)

```bash
knocode init      # build/rebuild the repository index (first use per repo) — safe to re-run
knocode doctor    # diagnose installation / daemon / index problems
knocode --version # check version
knocode status    # daemon status (if running)
```

## Rules

- The daemon serves one machine; scope every call with the **current workspace root** as `repository_path`.
- **Index missing/corrupt** (context calls fail or look stale): tell the user to run `knocode init` from the project root, or run it if you have a terminal.
- Reference returned **file paths** when explaining; still use `read`/`rg` for follow-up investigation.

## Troubleshooting

- **Tool call fails / server unreachable:** the daemon isn't running — start it with `knocode serve` (or ask the user to), then retry once.
- **Stale results:** run `knocode init` from the project root to rebuild the index.
- **Not working at all:** run `knocode doctor` and follow its output.
