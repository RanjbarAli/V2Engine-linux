#!/usr/bin/env bash
set -euo pipefail
VERSION="1.14.0"
SHA256="2375de6999f4f56ab46b4fc5ddf26a6aba1d3e61a0f4e7ddec2f4690457d5f63"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCHIVE="$(mktemp)"
WORK="$(mktemp -d)"
trap 'rm -f "$ARCHIVE"; rm -rf "$WORK"' EXIT
curl -fL --retry 3 -o "$ARCHIVE" "https://github.com/SagerNet/sing-box/releases/download/v${VERSION}/sing-box-${VERSION}-linux-amd64.tar.gz"
printf '%s  %s\n' "$SHA256" "$ARCHIVE" | sha256sum --check --status
tar -xzf "$ARCHIVE" -C "$WORK"
install -Dm755 "$WORK/sing-box-${VERSION}-linux-amd64/sing-box" "$ROOT/vendor/sing-box"
"$ROOT/vendor/sing-box" version
