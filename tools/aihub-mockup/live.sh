#!/bin/bash
# The real AI Hub plugin, drawn by the bar's renderer on a private Xvfb display,
# against a scratch nyx-agentd: three fake agents (one waiting for approval, one
# working, one done) and the real usage records (read only, quota checks off).
# Screenshots go to docs/mockups/live. Stops only what it started.
#   VIEWS="name=script ..."  scripts are ;-separated: open, action:CONTROL, submit:ID=TEXT
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
lab=${NYX_MOCKUP_LAB:-/tmp/nyx-agents-lab}
out=${NYX_MOCKUP_OUT:-$repo/docs/mockups/live}
mkdir -p "$lab/shots" "$out"
(cd "$repo" && cargo build --offline --quiet -p nyx-agentd -p nyx-agents -p nyx-ai-hub-plugin)
CARGO_TARGET_DIR=$lab/aihub-target cargo build --offline --quiet --manifest-path "$here/Cargo.toml"
bin=$repo/target/debug

scratch=$(mktemp -d /tmp/nyx-aihub-live.XXXX)
export NYX_AGENTS_RUNTIME_DIR=$scratch/run NYX_AGENTS_STATE_DIR=$scratch/state NYX_AGENTS_NO_DBUS=1
export NYX_AGENTS_CACHE_DIR=$lab/usage-cache
cfg=$scratch/config
mkdir -p "$cfg/nyx-agents"
b64() { printf %s "$1" | base64 -w0; }
# Each fake agent reports OSC 7501 status, then waits.
osc() { printf '\\033]7501;%s\\033\\\\' "$1"; }
agent() {
  local f=$scratch/$1.sh
  { echo '#!/bin/sh'; shift; for s in "$@"; do if [[ $s == sleep* ]]; then echo "$s"; else echo "printf '$(osc "$s")'"; fi; done; echo 'exec cat'; } >"$f"
  chmod +x "$f"
}
agent blocked "state=working:app=fake" "sleep 0.3" "state=blocked:kind=permission:app=fake:msg=$(b64 'Run cargo test --bin nyx-files parser?')"
agent working "state=working:app=fake"
agent done "state=working:app=fake" "sleep 0.3" "state=done:app=fake:msg=$(b64 'Updated the plugin docs')"
cat >"$cfg/nyx-agents/harnesses.toml" <<EOF
[[harness]]
id = "fake-blocked"
name = "Codex (fake)"
command = ["$scratch/blocked.sh"]

[[harness]]
id = "fake-working"
name = "Claude (fake)"
command = ["$scratch/working.sh"]

[[harness]]
id = "fake-done"
name = "OpenCode (fake)"
command = ["$scratch/done.sh"]
EOF

daemon=
ids=()
d=
xp=
cleanup() {
  [[ -n $xp ]] && kill "$xp" 2>/dev/null || true
  [[ -n $daemon ]] && kill "$daemon" 2>/dev/null || true
  for i in "${ids[@]}"; do pkill -f "nyx-agentd hold --id $i" -U "$(id -u)" --signal TERM 2>/dev/null || true; done
  rm -rf "$scratch"
}
trap cleanup EXIT
XDG_CONFIG_HOME=$cfg "$bin/nyx-agentd" serve 2>"$scratch/daemon.log" & daemon=$!
for _ in $(seq 50); do [[ -S $NYX_AGENTS_RUNTIME_DIR/agents.sock ]] && break; sleep 0.1; done
launch() { local id; id=$(XDG_CONFIG_HOME=$cfg "$bin/nyx-agents" launch "$1" --cwd "$2" --title "$3"); ids+=("$id"); }
launch fake-blocked "$HOME/Projects/nyx-files" "Fix the CSV parser"
launch fake-working "$HOME/Projects/nyx-agents" "Inspector polish"
launch fake-done "$HOME/Projects/nyx-bar" "Update the plugin docs"
sleep 1.2
"$bin/nyx-agents" ls

d=97; while [ -e /tmp/.X$d-lock ]; do d=$((d+1)); done
Xvfb :$d -screen 0 2560x1600x24 -nolisten tcp >/dev/null 2>&1 & xp=$!
sleep 1
for v in ${VIEWS:-agents=open usage=open\;action:tab:usage}; do
  name=${v%%=*}; script=${v#*=}
  steps=$(awk -F';' '{print NF}' <<<"$script")
  tab=agents; [[ $name == cc* ]] && tab=cc; [[ $name == cc-detail* ]] && tab=cc-detail
  env -u WAYLAND_DISPLAY DISPLAY=:$d GDK_BACKEND=x11 GDK_SCALE=2 NO_AT_BRIDGE=1 GDK_DEBUG=no-portals GTK_USE_PORTAL=0 \
    GSK_RENDERER=cairo XDG_CACHE_HOME=$lab/scratch/cache XDG_STATE_HOME=$lab/scratch/state \
    MOCK_TAB=$tab MOCK_PLUGIN=$bin/nyx-ai-hub-plugin MOCK_PLUGIN_DIR=$repo/apps/ai-hub-plugin MOCK_SCRIPT="$script" \
    MOCK_SCROLL=${MOCK_SCROLL:-0} MOCK_SCROLL_AT=${MOCK_SCROLL_AT:-$((3000 + 1000 * steps))} \
    dbus-run-session -- "$lab/aihub-target/debug/aihub-mockup" >"$lab/scratch/live-$name.log" 2>&1 & ap=$!
  sleep ${SHOT_AT:-$(( 4 + steps ))}
  import -display :$d -window root "$lab/shots/live-$name-2x.png"
  kill $ap 2>/dev/null; wait $ap 2>/dev/null || true
  pkill -f "$bin/nyx-ai-hub-plugin" -U "$(id -u)" 2>/dev/null || true
  rg -n 'panicked' -A2 "$lab/scratch/live-$name.log" && exit 1
  case $name in cc*) crop=840x1560+1720+0 ;; *) crop=1240x1560+1320+0 ;; esac
  magick "$lab/shots/live-$name-2x.png" -crop $crop +repage -trim +repage -bordercolor '#0d1117' -border 8 -resize 50% "$out/$name.png"
  echo "$out/$name.png"
done
