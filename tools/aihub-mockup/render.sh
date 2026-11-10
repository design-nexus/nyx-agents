#!/bin/bash
# Renders the AI Hub mockup views on a private Xvfb display (no compositor calls) and
# writes cropped screenshots to docs/mockups. VIEWS="name[:scroll] ..." picks views;
# scroll is 0-1 through a long popover.
set -e
here=$(cd "$(dirname "$0")" && pwd)
lab=${NYX_MOCKUP_LAB:-/tmp/nyx-agents-lab}
out=${NYX_MOCKUP_OUT:-$here/../../docs/mockups}
mkdir -p "$lab/scratch/cache" "$lab/scratch/state" "$lab/shots" "$out"
python3 "$here/gen_views.py"
CARGO_TARGET_DIR=$lab/aihub-target cargo build --offline --quiet --manifest-path "$here/Cargo.toml"
d=97; while [ -e /tmp/.X$d-lock ]; do d=$((d+1)); done
Xvfb :$d -screen 0 2560x1600x24 -nolisten tcp >/dev/null 2>&1 & xp=$!
trap 'kill $xp 2>/dev/null' EXIT
sleep 1
for v in ${VIEWS:-agents usage sessions local}; do
  name=${v%%:*}; scroll=${v#*:}; [ "$scroll" = "$v" ] && scroll=0
  shot=$name; [ "$scroll" != 0 ] && shot=$name-$scroll
  env -u WAYLAND_DISPLAY DISPLAY=:$d GDK_BACKEND=x11 GDK_SCALE=2 NO_AT_BRIDGE=1 GDK_DEBUG=no-portals GTK_USE_PORTAL=0 \
    GSK_RENDERER=cairo XDG_CACHE_HOME=$lab/scratch/cache XDG_STATE_HOME=$lab/scratch/state MOCK_TAB=$name MOCK_SCROLL=$scroll \
    dbus-run-session -- "$lab/aihub-target/debug/aihub-mockup" >"$lab/scratch/$shot.log" 2>&1 & ap=$!
  sleep 2.5
  import -display :$d -window root "$lab/shots/$shot-2x.png"
  kill $ap 2>/dev/null; wait $ap 2>/dev/null || true
  if rg -q panicked "$lab/scratch/$shot.log"; then rg -A1 panicked "$lab/scratch/$shot.log"; exit 1; fi
  case $name in
    cc*) crop=840x1560+1720+0 ;;
    item) crop=520x64+2040+0 ;;
    *) crop=1240x1560+1320+0 ;;
  esac
  magick "$lab/shots/$shot-2x.png" -crop $crop +repage -trim +repage -bordercolor '#0d1117' -border 8 -resize 50% "$out/ai-hub-$shot.png"
  echo "$out/ai-hub-$shot.png"
done
