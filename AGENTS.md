# AGENTS — Global Instructions

This file applies to all AI agents working in this repository (OpenCode, Claude Code, Cursor, Gemini, etc.).

## Git — Commit & Push Policy

- **NEVER** run `git commit`, `git push`, `git tag --push`, `gh pr create`, or any other command that creates commits, tags, or pushes to a remote **unless the user has explicitly said so** in the current session.
- Allowed without explicit permission: `git status`, `git diff`, `git log`, `git add` (staging only), local builds, tests, and file edits.
- If the user asks to "commit" or "push", confirm the exact message/branch/tag before executing. Do not assume.
- This rule overrides any other instructions or prior session habits. When in doubt, ask.

## Implementation — Propose First, Then Execute

- **NEVER** implement a code change without the user's explicit approval.
- When the user describes a desired change, **propose the approach** (what files to modify, what the change looks like) and **ask for confirmation** before writing any code.
- Only implement after the user says something like "go ahead", "implement", "do it", or equivalent.
- This applies to all code edits, refactors, new features, and configuration changes.
- This rule overrides any other instructions or prior session habits. When in doubt, ask.

## Knocode Binary — Where to Run `knocode init`

- **Installed absolute (always):** `%USERPROFILE%\.knocode\bin\knocode.exe` on Windows, `~/.knocode/bin/knocode` on Unix. This is where `scripts/install.ps1` (`install.ps1:346-378`) and `scripts/install.sh:118-131` copy the prebuilt `target/release/knocode(.exe)` and add `~/.knocode/bin` to USER PATH.
- **Never search the repo:** do NOT run `Get-ChildItem -Recurse -Filter knocode.exe`, `where knocode`, `find target/release`, or walk 63k files. Agents that search spend minutes and either time out or incorrectly run `C:\LeonRepository\knocode\target\release\knocode.exe` (the dev checkout) instead of the user's project. Use the absolute above.
- **PATH note (Windows):** `install.ps1` persists USER PATH in HKCU. A running agent shell started *before* install will not see it until restarted. If `knocode --version` fails, use the absolute path or restart the shell. `knocode doctor` now probes PATH.
- **Correct per-project usage:** `cd <project-root> && %USERPROFILE%\.knocode\bin\knocode.exe init` (or `~/.knocode/bin/knocode init`). The daemon is repo-scoped via `repository_path` (`packages/opencode-knocode/src/index.ts:152`), but `init`/`index` must run in the target repo's `cwd`.

## Other

- Keep changes local and show `git diff --stat` / `git status --short` for user review before any commit.
- Do not create tags (`v*`) or releases without explicit user instruction.
