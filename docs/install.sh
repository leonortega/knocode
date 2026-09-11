#!/usr/bin/env bash
# Knocode Linux/macOS installer bootstrap.
#
# This is a thin bootstrapper hosted on GitHub Pages.
# It downloads the full installer from the latest GitHub Release and executes it.
#
# One-liner:
#   curl -fsSL https://leonortega.github.io/knocode/install.sh | bash

set -euo pipefail

REPO="leonortega/knocode"

echo "[knocode] Resolving latest release..."

# Resolve latest tag
TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep '"tag_name"' | sed 's/.*"tag_name": *"//;s/".*//')

if [ -z "$TAG" ]; then
  echo "[knocode] Could not resolve latest release" >&2
  exit 1
fi

echo "[knocode] Latest release: $TAG"

# Check for the installer script in the release assets
INSTALLER_URL="https://github.com/$REPO/releases/download/$TAG/installers/knocode-install.sh"
HTTP_CODE=$(curl -fsSL -o /dev/null -w "%{http_code}" "$INSTALLER_URL" 2>/dev/null || echo "404")

if [ "$HTTP_CODE" = "200" ]; then
  echo "[knocode] Downloading installer from $TAG..."
  curl -fsSL "$INSTALLER_URL" | bash
else
  echo "[knocode] No Linux/macOS installer found in release $TAG."
  echo "[knocode] Download the binary manually from:"
  echo "  https://github.com/$REPO/releases/tag/$TAG"
  exit 1
fi
