#!/usr/bin/env bash
set -euo pipefail
DEST="${HOME:?}/.local"
rm -f "$DEST/bin/kinseat-client" "$DEST/share/applications/kinseat-client.desktop"
for size in 16 24 32 48 64 128 256; do
    rm -f "$DEST/share/icons/hicolor/${size}x${size}/apps/kinseat-client.png"
done
printf 'Kinseat Client retiré ; les réglages personnels sont conservés.\n'
