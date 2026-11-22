#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="${HOME:?}/.local"
mkdir -p "$DEST/bin" "$DEST/share/applications"
install -m755 "$HERE/kinseat-client" "$DEST/bin/kinseat-client"
install -m644 "$HERE/kinseat-client.desktop" "$DEST/share/applications/kinseat-client.desktop"
for icon in "$HERE"/icons/kinseat-*.png; do
    size="${icon##*/kinseat-}"
    size="${size%.png}"
    mkdir -p "$DEST/share/icons/hicolor/${size}x${size}/apps"
    install -m644 "$icon" "$DEST/share/icons/hicolor/${size}x${size}/apps/kinseat-client.png"
done
printf 'Installé dans %s ; ajoutez %s/bin à PATH si nécessaire.\n' "$DEST" "$DEST"
