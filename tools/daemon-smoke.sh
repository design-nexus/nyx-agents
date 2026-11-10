#!/usr/bin/env bash
# End-to-end check of nyx-agentd with scratch folders: launch a fake agent
# that reports OSC 7501 status, restart the manager and confirm it
# reattaches, observe the terminal, stop and forget. Runs nothing real and
# leaves nothing running. Run under dbus-run-session with NYX_SMOKE_DBUS=1
# to check the D-Bus actions on a private bus.
set -euo pipefail
here=$(cd "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
bin=${BIN:-$here/target/debug}
scratch=$(mktemp -d /tmp/nyx-agents-smoke.XXXX)
[[ ${NYX_SMOKE_DBUS:-} == 1 ]] || export NYX_AGENTS_NO_DBUS=1
export NYX_AGENTS_RUNTIME_DIR=$scratch/run NYX_AGENTS_STATE_DIR=$scratch/state XDG_CONFIG_HOME=$scratch/config
# Usage scans made-up records, never the real ones.
export NYX_AGENTS_CACHE_DIR=$scratch/cache CLAUDE_CONFIG_DIR=$scratch/claude CODEX_HOME=$scratch/codex
mkdir -p "$XDG_CONFIG_HOME/nyx-agents"
cat >"$XDG_CONFIG_HOME/nyx-agents/harnesses.toml" <<'EOF'
[[harness]]
id = "fake"
name = "Fake agent"
command = ["sh", "-c", "printf 'fake agent ready\\r\\n'; printf '\\033]7501;state=working:app=fake\\033\\\\'; sleep 1; printf 'did the work\\r\\n\\033]7501;state=done:app=fake:msg=RmluaXNoZWQ=\\033\\\\'; exec cat"]
EOF
daemon=
cleanup() {
  [[ -n $daemon ]] && kill "$daemon" 2>/dev/null || true
  pkill -f "nyx-agentd hold --id" -U "$(id -u)" --signal TERM 2>/dev/null || true
  rm -rf "$scratch"
}
trap cleanup EXIT
start() { "$bin/nyx-agentd" serve & daemon=$!; for _ in $(seq 50); do [[ -S $NYX_AGENTS_RUNTIME_DIR/agents.sock ]] && break; sleep 0.1; done; }
cli() { "$bin/nyx-agents" "$@"; }
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
fail=0

start
cli harnesses | grep -q '^fake ' && echo "ok   generic harness listed"
id=$(cli launch fake --cwd "$here" --title smoke)
sleep 0.5
check "running and working" '[[ $(cli status "$id" --json | jq -r ".lifecycle + \" \" + .status.activity") == "running working" ]]'
sleep 1.2
check "done after one turn" '[[ $(cli status "$id" --json | jq -r ".status.activity, .status.turn, .status.message" | tr "\n" " ") == "done 1 Finished " ]]'
check "branch from git" '[[ -n $(cli status "$id" --json | jq -r ".worktree.branch // empty") ]]'
if [[ -n ${DBUS_SESSION_BUS_ADDRESS:-} && ${NYX_SMOKE_DBUS:-} == 1 ]]; then
  dbus() { gdbus call --session --dest io.github.design_nexus.nyx.AgentManager --object-path /io/github/design_nexus/nyx/AgentManager --method "io.github.design_nexus.nyx.Control.$1" "${@:2}"; }
  check "control describes its actions" '[[ $(dbus Describe) == *agents.list_sessions* ]]'
  check "control lists the session" '[[ $(dbus Call agents.list_sessions "{}") == *"$id"* ]]'
fi
check "hub counts it" '[[ $(jq -r ".agents, .done" "$NYX_AGENTS_RUNTIME_DIR/hub.json" | tr "\n" " ") == "1 1 " ]]'

now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
mkdir -p "$CLAUDE_CONFIG_DIR/projects/-smoke"
printf '%s\n' \
  "{\"type\":\"user\",\"uuid\":\"u1\",\"timestamp\":\"$now\",\"cwd\":\"/smoke\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}" \
  "{\"type\":\"assistant\",\"timestamp\":\"$now\",\"requestId\":\"r1\",\"message\":{\"id\":\"m1\",\"role\":\"assistant\",\"model\":\"claude-opus-5-5\",\"content\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}}" \
  >"$CLAUDE_CONFIG_DIR/projects/-smoke/s1.jsonl"
check "usage reads the records" '[[ $(cli usage claude --refresh --json | jq -r ".[0].today.prompts, .[0].today.tokens, .[0].quota.enabled" | tr "\n" " ") == "1 7 false " ]]'
check "hub carries usage" '[[ $(jq -r ".usage[0].harness, .usage[0].today_prompts" "$NYX_AGENTS_RUNTIME_DIR/hub.json" | tr "\n" " ") == "claude 1 " ]]'

kill "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=
check "agent outlives the manager" 'pgrep -f "nyx-agentd hold --id $id" >/dev/null'
start
sleep 0.3
check "manager reattaches" '[[ $(cli status "$id" --json | jq -r .lifecycle) == running ]]'

screen=$(timeout 1 "$bin/nyx-agents" attach "$id" --observe </dev/null 2>&1 || true)
[[ -n ${DEBUG:-} ]] && printf '%q\n' "$screen" >&2
check "observer sees the screen" '[[ $screen == *"did the work"* ]]'
timeout 1 "$bin/nyx-agents" attach "$id" --observe <<<"should not arrive" >/dev/null 2>&1 || true
screen=$(timeout 1 "$bin/nyx-agents" attach "$id" --observe </dev/null 2>/dev/null || true)
check "observer input is ignored" '[[ $screen != *"should not arrive"* ]]'

cli stop "$id"
sleep 0.6
check "stopped" '[[ $(cli status "$id" --json | jq -r .lifecycle) == exited ]]'
check "history kept" '[[ -s $NYX_AGENTS_STATE_DIR/sessions/$id/screen.snap ]]'
cli forget "$id"
check "forgotten" '! cli status "$id" >/dev/null 2>&1'
sleep 2.5
check "no holders left" '! pgrep -f "nyx-agentd hold --id $id" >/dev/null'
exit $fail
