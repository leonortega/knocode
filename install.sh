#!/usr/bin/env bash
# Knocode Linux/macOS installer bootstrap.
#
# Downloads the full installer (knocode-install.sh) from the latest GitHub
# Release into a temp file and executes it as a file.
#
#   curl -fsSL https://raw.githubusercontent.com/leonortega/knocode/main/install.sh | bash
#
# Pinned version (args are forwarded to the full installer):
#   curl -fsSL https://raw.githubusercontent.com/leonortega/knocode/main/install.sh | bash -s -- --version <x.y.z>
#
# Env override (works via pipe too): KNOCODE_VERSION=x.y.z
set -euo pipefail

REPO="leonortega/knocode"

# Pinned version via --version X.Y.Z / --version=X.Y.Z or KNOCODE_VERSION.
VERSION=""
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:-}"; args+=("$1" "${2:-}"); shift 2;;
    --version=*) VERSION="${1#--version=}"; args+=("$1"); shift;;
    *) args+=("$1"); shift;;
  esac
done
if [ -z "$VERSION" ] && [ -n "${KNOCODE_VERSION:-}" ]; then VERSION="$KNOCODE_VERSION"; fi

# Resolve the release asset URL. Prefer the releases/latest redirect (no API
# rate limit); fall back to the GitHub API.
ASSET_URL=""
if [ -n "$VERSION" ]; then
  VER="${VERSION#v}"
  case "$VER" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) echo "[knocode] invalid --version '$VERSION' (expected x.y.z)" >&2; exit 1;;
  esac
  ASSET_URL="https://github.com/$REPO/releases/download/v$VER/knocode-install.sh"
  echo "[knocode] Pinned version: $VER"
else
  TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null | sed -n 's#.*/releases/tag/##p' || true)"
  if [ -z "$TAG" ]; then
    TAG="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep '"tag_name"' | sed 's/.*"tag_name": *"//;s/".*//' || true)"
  fi
  if [ -n "$TAG" ]; then
    echo "[knocode] Latest release: $TAG"
    ASSET_URL="https://github.com/$REPO/releases/download/$TAG/knocode-install.sh"
  else
    echo "[knocode] release lookup failed, falling back to latest URL" >&2
    ASSET_URL="https://github.com/$REPO/releases/latest/download/knocode-install.sh"
  fi
fi

# Download to a temp file and sanity-check before running (never pipe a
# half-downloaded stream into a shell).
TMP_FILE="$(mktemp /tmp/knocode-install-XXXXXX.sh)"
trap 'rm -f "$TMP_FILE"' EXIT
if ! curl -fsSL "$ASSET_URL" -o "$TMP_FILE"; then
  echo "[knocode] download failed ($ASSET_URL) - download manually from https://github.com/$REPO/releases" >&2
  exit 1
fi
if [ ! -s "$TMP_FILE" ] || [ "$(wc -c < "$TMP_FILE")" -lt 500 ]; then
  echo "[knocode] downloaded installer looks truncated - download manually from https://github.com/$REPO/releases" >&2
  exit 1
fi
if ! bash -n "$TMP_FILE"; then
  echo "[knocode] downloaded installer failed syntax check - download manually from https://github.com/$REPO/releases" >&2
  exit 1
fi

# Empty-array safe on bash 3.2 (macOS): "${args[@]}" with set -u fails when empty.
if [ "${#args[@]}" -gt 0 ]; then
  bash "$TMP_FILE" "${args[@]}"
else
  bash "$TMP_FILE"
fi
