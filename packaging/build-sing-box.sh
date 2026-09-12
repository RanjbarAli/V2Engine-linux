#!/usr/bin/env bash
set -euo pipefail

VERSION="1.14.0"
SOURCE_SHA256="87baf6852e37941cbe40bdd94bec81c957c88a56751cecd6bbf0e6108bc69398"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCHIVE="$(mktemp)"
WORK="$(mktemp -d)"
trap 'rm -f "$ARCHIVE"; rm -rf "$WORK"' EXIT

curl -fL --retry 3 -o "$ARCHIVE" \
  "https://github.com/SagerNet/sing-box/archive/refs/tags/v${VERSION}.tar.gz"
printf '%s  %s\n' "$SOURCE_SHA256" "$ARCHIVE" | sha256sum --check --status
tar -xzf "$ARCHIVE" -C "$WORK"

(
  cd "$WORK/sing-box-${VERSION}"
  export CGO_ENABLED=0 GOOS=linux GOARCH=amd64 GOTOOLCHAIN=local
  go build \
    -trimpath \
    -tags with_utls \
    -ldflags "-X github.com/sagernet/sing-box/constant.Version=${VERSION} -s -w -buildid=" \
    -o "$ROOT/vendor/sing-box" \
    ./cmd/sing-box
)

chmod 0755 "$ROOT/vendor/sing-box"
"$ROOT/vendor/sing-box" version
