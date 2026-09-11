#!/bin/bash
# Knocode readiness wait — shared library for Claude Code hooks.
#
# The daemon's HTTP health/metrics listener binds BEFORE the initial repository
# index runs, so during a cold start `GET /health` answers `{"state":"indexing"}`
# and `POST /hook` returns 503 (`daemon_indexing`). Hooks source this file and call
# `knocode_wait_ready` before their first request so the first prompt of a session
# gets context instead of an instant 503 passthrough.
#
# Semantics (parity with the opencode plugin's waitForDaemonReady):
#   return 0 — daemon reports ready, OR the HTTP listener answered 200 without a
#              parseable state (a live daemon is better than a strict one)
#   return 1 — daemon UNREACHABLE (connection refused — not running), returned fast
#              so a missing daemon never stalls a hook for the full budget
#   return 1 — budget (KNOCODE_READY_TIMEOUT_MS, default 10s) expired while the
#              daemon kept indexing
# Fail-open: callers proceed with the POST regardless (`knocode_wait_ready || true`).
#
# Usage (from a hook script in this directory):
#   SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
#   # shellcheck source=knocode-ready.sh
#   source "${SCRIPT_DIR}/knocode-ready.sh"
#   knocode_wait_ready || true
#
# Poll interval is 1s (portable `sleep`); the budget is derived from the same
# KNOCODE_READY_TIMEOUT_MS env var the JS adapters use.

knocode_wait_ready() {
  local url="${KNOCODE_DAEMON_URL:-http://127.0.0.1:9527}"
  local budget_ms="${KNOCODE_READY_TIMEOUT_MS:-10000}"
  local budget_s=$(((budget_ms + 999) / 1000))
  [ "$budget_s" -lt 1 ] && budget_s=1

  local deadline=$(( $(date +%s) + budget_s ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    local body rc
    # Per-poll cap of 2s: a local daemon answers in ms; a stall means it is not healthy.
    body=$(curl -sS --connect-timeout 1 --max-time 2 "${url}/health" 2>/dev/null)
    rc=$?
    if [ "$rc" -ne 0 ] || [ -z "$body" ]; then
      # Unreachable (connection refused / aborted) — daemon not running. Don't burn the budget.
      return 1
    fi

    # Dependency-free state parse (no jq/python3 required): only an explicit
    # "state":"indexing" keeps us polling; a 200 without it means a live daemon.
    local state
    state=$(echo "$body" | sed -nE 's/.*"state"[[:space:]]*:[[:space:]]*"([a-z]+)".*/\1/p' | head -n1)
    if [ -z "$state" ] || [ "$state" = "ready" ]; then
      # 200 from a live daemon — ready (or at least serving).
      return 0
    fi
    # Reachable but still indexing — keep polling until the deadline.
    sleep 1
  done
  return 1
}
