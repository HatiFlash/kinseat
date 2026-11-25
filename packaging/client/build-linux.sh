#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/app/target}"
OUT="${OUT:-$ROOT/dist/client}"
mkdir -p "$OUT"
cd "$ROOT/app"
cargo build --release -p kinseat-client
BIN="$CARGO_TARGET_DIR/release/kinseat-client"
VERSION=0.3.22
MAX_GLIBC="${KINSEAT_MAX_GLIBC:-2.35}"
GLIBC_NEEDED="$(objdump -T "$BIN" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n1)"
if [ -z "$GLIBC_NEEDED" ]; then
  echo "Erreur : impossible de déterminer la version GLIBC requise par $BIN" >&2
  exit 1
fi
if [ "$(printf '%s\n%s\n' "$GLIBC_NEEDED" "$MAX_GLIBC" | sort -V | tail -n1)" != "$MAX_GLIBC" ]; then
  echo "Erreur : le binaire requiert GLIBC_$GLIBC_NEEDED, au-dessus du plafond KINSEAT_MAX_GLIBC=$MAX_GLIBC. Compiler sur un système plus ancien (ubuntu-22.04)." >&2
  exit 1
fi
printf 'GLIBC requise : %s (plafond %s)\n' "$GLIBC_NEEDED" "$MAX_GLIBC"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/kinseat-client-deb.XXXXXX")"
PORTABLE="$(mktemp -d "${TMPDIR:-/tmp}/kinseat-client-portable.XXXXXX")"
trap 'rm -rf "$STAGE" "$PORTABLE"' EXIT
mkdir -p "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/share/applications" "$STAGE/usr/share/icons/hicolor" "$PORTABLE/icons"
install -m755 "$BIN" "$STAGE/usr/bin/kinseat-client"
install -m755 "$BIN" "$PORTABLE/kinseat-client"
for size in 16 24 32 48 64 128 256; do
  mkdir -p "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps"
  install -m644 "$ROOT/app/kinseat-client/assets/kinseat-${size}.png" "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/kinseat-client.png"
  install -m644 "$ROOT/app/kinseat-client/assets/kinseat-${size}.png" "$PORTABLE/icons/kinseat-${size}.png"
done
install -m644 "$ROOT/packaging/client/kinseat-client.desktop" "$STAGE/usr/share/applications/kinseat-client.desktop"
install -m644 "$ROOT/packaging/client/kinseat-client.desktop" "$PORTABLE/kinseat-client.desktop"
printf 'Package: kinseat-client\nVersion: %s\nArchitecture: amd64\nMaintainer: Kinseat <hatiflash@users.noreply.github.com>\nDepends: libc6 (>= %s), libgl1, libegl1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libx11-6, libxcursor1, libxi6, libxrandr2\nRecommends: moonlight-qt, libvulkan1, mesa-vulkan-drivers\nSection: games\nPriority: optional\nDescription: Kinseat Client - jouer sur une place du PC Kinseat\n Trouve les places du PC hôte Kinseat, les associe par code et lance le streaming\n avec Moonlight (moonlight-qt, Flatpak com.moonlight_stream.Moonlight ou AppImage).\n' "$VERSION" "$GLIBC_NEEDED" > "$STAGE/DEBIAN/control"
install -m755 "$ROOT/packaging/client/install.sh" "$ROOT/packaging/client/uninstall.sh" "$PORTABLE/"
mkdir -p "$STAGE/usr/share/doc/kinseat-client"
install -m644 "$ROOT/LICENSE" "$STAGE/usr/share/doc/kinseat-client/copyright"
install -m644 "$ROOT/LICENSE" "$PORTABLE/LICENSE"
dpkg-deb --build --root-owner-group "$STAGE" "$OUT/kinseat-client_${VERSION}_amd64.deb"
tar -C "$PORTABLE" -czf "$OUT/kinseat-client_${VERSION}_linux-x64.tar.gz" .
dpkg-deb --info "$OUT/kinseat-client_${VERSION}_amd64.deb"
dpkg-deb --contents "$OUT/kinseat-client_${VERSION}_amd64.deb"
if command -v lintian >/dev/null; then lintian "$OUT/kinseat-client_${VERSION}_amd64.deb"; else printf 'lintian indisponible\n'; fi
sha256sum "$OUT/kinseat-client_${VERSION}_amd64.deb" "$OUT/kinseat-client_${VERSION}_linux-x64.tar.gz"
