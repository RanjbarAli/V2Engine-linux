#!/usr/bin/env bash
set -euo pipefail
VERSION="${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$(dirname "$0")/../Cargo.toml" | head -n1)}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([~+-][0-9A-Za-z.-]+)?$ ]]; then
  echo "Invalid package version: $VERSION" >&2
  exit 2
fi
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="$ROOT/dist/V2Engine_${VERSION}_amd64"
rm -rf "$STAGE"
install -Dm755 "$ROOT/target/release/v2engine" "$STAGE/usr/bin/v2engine"
install -Dm755 "$ROOT/target/release/v2engine-helper" "$STAGE/usr/lib/v2engine/v2engine-helper"
install -Dm755 "$ROOT/vendor/sing-box" "$STAGE/usr/lib/v2engine/sing-box"
install -Dm644 "$ROOT/packaging/io.github.ranjbarali.V2Engine.desktop" "$STAGE/usr/share/applications/io.github.ranjbarali.V2Engine.desktop"
install -Dm644 "$ROOT/packaging/io.github.ranjbarali.V2Engine.png" "$STAGE/usr/share/icons/hicolor/512x512/apps/io.github.ranjbarali.V2Engine.png"
install -Dm644 "$ROOT/packaging/io.github.ranjbarali.V2Engine.policy" "$STAGE/usr/share/polkit-1/actions/io.github.ranjbarali.V2Engine.policy"
install -Dm644 "$ROOT/packaging/copyright" "$STAGE/usr/share/doc/v2engine/copyright"
install -Dm644 "$ROOT/packaging/lintian-overrides" "$STAGE/usr/share/lintian/overrides/v2engine"
install -Dm644 "$ROOT/packaging/v2engine.1" "$STAGE/usr/share/man/man1/v2engine.1"
gzip -n -9 "$STAGE/usr/share/man/man1/v2engine.1"
install -Dm644 "$ROOT/packaging/changelog" "$STAGE/usr/share/doc/v2engine/changelog"
gzip -n -9 "$STAGE/usr/share/doc/v2engine/changelog"
install -d "$STAGE/DEBIAN"
sed "s/@VERSION@/$VERSION/" "$ROOT/packaging/control.in" > "$STAGE/DEBIAN/control"
install -Dm755 "$ROOT/packaging/postrm" "$STAGE/DEBIAN/postrm"
dpkg-deb --root-owner-group --build "$STAGE" "$ROOT/dist/V2Engine_${VERSION}_amd64.deb"
