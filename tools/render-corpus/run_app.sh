#!/bin/bash
# Render corpus mails through the app (examples/mail_render) and compare each with its Chrome
# reference.
#
# usage: run_app.sh [id ...]            (default: every mail that has a reference render)
#   OUT=dir        where <id>.app.png/.app.json/.cmp.* go (default: the reference dir). Give each
#                  branch its own OUT to compare two builds with cmp_agg.sh.
#   NO_IMAGES=1    skip the side-by-side pages (metrics only; much faster to compare)
set -u
TOOLS="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$TOOLS/../.." && pwd)"
WORK="${RENDER_CORPUS:-$REPO/target/render-corpus}"
REF="$WORK/out"
CORPUS="$WORK/corpus"
OUT="${OUT:-$REF}"
mkdir -p "$OUT"
( cd "$REPO" && cargo build -q --example mail_render ) || { echo "build failed"; exit 1; }
BIN="${CARGO_TARGET_DIR:-$REPO/target}/debug/examples/mail_render"
if [ $# -gt 0 ]; then IDS="$*"; else IDS=$(ls "$REF" | sed -n 's/\.chrome\.json$//p' | sort -n); fi
if [ "$OUT" != "$REF" ]; then
  for id in $IDS; do for ext in chrome.png chrome.json manifest.json; do ln -sf "$REF/$id.$ext" "$OUT/$id.$ext"; done; done
fi
render() {
  id="$1"
  ( "$BIN" "$CORPUS/$id.html" "$REF/$id.manifest.json" "$OUT/$id" > "$OUT/$id.app.log" 2>&1 ) &
  # A hung layout must not stall the batch.
  pid=$!; ( sleep 90; kill -9 $pid 2>/dev/null ) & guard=$!
  wait $pid; rc=$?; kill $guard 2>/dev/null; wait $guard 2>/dev/null
  [ $rc -ne 0 ] && echo "$id exit $rc"
  return 0
}
export -f render; export BIN CORPUS REF OUT
echo $IDS | tr ' ' '\n' | xargs -P 6 -I{} bash -c 'render {}'
PANICS=$(grep -l panicked "$OUT"/*.app.log 2>/dev/null | wc -l | tr -d ' ')
[ "$PANICS" != "0" ] && echo "$PANICS mails panicked in layout (grep -l panicked $OUT/*.app.log)"
( cd "$TOOLS" && node compare.mjs "$OUT" ${NO_IMAGES:+--no-images} "$@" )
