#!/usr/bin/env bash
# Captures de toutes les pages de l'application Hôte en mode démonstration (données fictives),
# à plusieurs tailles et zooms, pour repérer les défauts d'affichage avant une publication.
# Usage : scripts/ui-screenshots.sh <binaire kinseat> <dossier de sortie>
# Linux : nécessite Xvfb (lancé automatiquement si DISPLAY est vide).
set -euo pipefail
BIN=${1:?binaire kinseat}
OUT=${2:?dossier de sortie}
mkdir -p "$OUT"
XV=
if [ -z "${DISPLAY:-}" ]; then
  export DISPLAY=:97
  Xvfb "$DISPLAY" -screen 0 2600x1800x24 >/dev/null 2>&1 &
  XV=$!
  sleep 2
fi
fail=0
for spec in 1280x800:1.0 1024x700:1.25 1600x900:1.5; do
  size=${spec%%:*}; zoom=${spec##*:}
  for page in overview seats members devices library settings notice preview join; do
    file="$OUT/$size-$zoom-$page.png"
    if [ "$page" = join ]; then args=(--demo-join); else args=(--demo --page "$page"); fi
    if ! timeout 60 "$BIN" "${args[@]}" --size "$size" --zoom "$zoom" --screenshot "$file" >/dev/null 2>&1 || [ ! -s "$file" ]; then
      echo "ÉCHEC : $page $size zoom $zoom"; fail=1
    fi
  done
done
[ -n "$XV" ] && kill "$XV" || true
ls "$OUT" | wc -l | xargs echo "captures :"
exit $fail
