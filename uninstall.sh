#!/usr/bin/env bash
# Knocode Linux/macOS uninstaller bootstrap.
#
# Downloads the full uninstaller (uninstall.sh) from the latest GitHub
# Release into a temp file and executes it as a file.
#
#   curl -fsSL https://raw.githubusercontent.com/leonortega/knocode/main/uninstall.sh | bash
#
# Non-interactive (args are forwarded to the full uninstaller):
#   curl -fsSL https://raw.githubusercontent.com/leonortega/knocode/main/uninstall.sh | bash -s -- --force
#
# Env override (works via pipe too): KNOCODE_UNINSTALL_FORCE=1 skips prompts.
set -euo pipefail

REPO="leonortega/knocode"

args=()
for arg in "$@"; do args+=("$arg"); done
FORCE_ENV=""
if [ -n "${KNOCODE_UNINSTALL_FORCE:-}" ]; then FORCE_ENV="--force"; fi

# Resolve the uninstaller asset from the latest release. Prefer the
# releases/latest redirect (no API rate limit); fall back to the GitHub API.
ASSET_URL=""
TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null | sed -n 's#.*/releases/tag/##p' || true)"
if [ -z "$TAG" ]; then
  TAG="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep '"tag_name"' | sed 's/.*"tag_name": *"//;s/".*//' || true)"
fi
if [ -n "$TAG" ]; then
  echo "[knocode] Latest release: $TAG"
  ASSET_URL="https://github.com/$REPO/releases/download/$TAG/uninstall.sh"
else
  echo "[knocode] release lookup failed, falling back to latest URL" >&2
  ASSET_URL="https://github.com/$REPO/releases/latest/download/uninstall.sh"
fi

# Download to a temp file and sanity-check before running (never pipe a
# half-downloaded stream into a shell).
TMP_FILE="$(mktemp /tmp/knocode-uninstall-XXXXXX.sh)"
trap 'rm -f "$TMP_FILE"' EXIT
if ! curl -fsSL "$ASSET_URL" -o "$TMP_FILE"; then
  echo "[knocode] download failed ($ASSET_URL) - download manually from https://github.com/$REPO/releases" >&2
  exit 1
fi
if [ ! -s "$TMP_FILE" ] || [ "$(wc -c < "$TMP_FILE")" -lt 500 ]; then
  echo "[knocode] downloaded uninstaller looks truncated - download manually from https://github.com/$REPO/releases" >&2
  exit 1
fi
if ! bash -n "$TMP_FILE"; then
  echo "[knocode] downloaded uninstaller failed syntax check - download manually from https://github.com/$REPO/releases" >&2
  exit 1
fi

# Empty-array safe on bash 3.2 (macOS): "${args[@]}" with set -u fails when empty.
if [ -n "$FORCE_ENV" ] && [ "${#args[@]}" -gt 0 ]; then
  bash "$TMP_FILE" --force "${args[@]}"
elif [ -n "$FORCE_ENV" ]; then
  bash "$TMP_FILE" --force
elif [ "${#args[@]}" -gt 0 ]; then
  bash "$TMP_FILE" "${args[@]}"
else
  bash "$TMP_FILE"
fi
