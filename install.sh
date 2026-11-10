#!/usr/bin/env bash
# Install Nyx Agents: nyx-agentd (the session manager) and nyx-agents, plus
# the user units that start the manager on first use.
#
#   ./install.sh [agents]     build this checkout and install it
#
# NYX_AGENTS_NO_ENABLE=1 installs without enabling the socket.
set -euo pipefail

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31m==>\033[0m %s\n' "$*" >&2; exit 1; }

case ${1:-agents} in
  agents | all | --*) ;;
  -h | --help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) die "This repository installs only agents (Nyx Agents)." ;;
esac

src=$(cd "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
[[ -f $src/Cargo.toml && -d $src/crates ]] || die "Run this from a Nyx Agents checkout."
command -v cargo >/dev/null || die "Rust (cargo) is needed."
command -v zig >/dev/null || die "Zig 0.17 is needed to build libghostty-vt."

if ! (cd "$src/vendor/libghostty-vt" && sha256sum --quiet --ignore-missing -c SHA256SUMS 2>/dev/null) \
  || [[ ! -f $src/vendor/libghostty-vt/libghostty-vt-1.0.0.tar.gz ]]; then
  say "Fetching libghostty-vt 1.0.0 (about 10 MB)"
  "$src/tools/fetch-libghostty-vt.sh"
fi

say "Building Nyx Agents"
target=${CARGO_TARGET_DIR:-$src/target}
(cd "$src" && cargo build --release --locked -p nyx-agentd -p nyx-agents -p agents-glimpse)

bin="$HOME/.local/bin"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"
mkdir -p "$bin" "$units"
say "Installing nyx-agentd and nyx-agents"
for b in nyx-agentd nyx-agents; do
  install -m 755 "$target/release/$b" "$bin/$b.new"
  mv -f "$bin/$b.new" "$bin/$b"
done
install -m 644 "$src/data/nyx-agentd.socket" "$src/data/nyx-agentd.service" "$units/"
if [[ -f $src/data/io.github.design_nexus.nyx.Agents.desktop ]]; then
  mkdir -p "$apps" "$icons"
  install -m 644 "$src/data/io.github.design_nexus.nyx.Agents.desktop" "$apps/"
  install -m 644 "$src/data/io.github.design_nexus.nyx.Agents.svg" "$icons/"
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$apps" || true
fi

# The Glimpse plugin, for Glimpse to install with your consent:
#   nyx-glimpse plugins install-local ~/.local/share/nyx-agents/glimpse
plug="${XDG_DATA_HOME:-$HOME/.local/share}/nyx-agents/glimpse"
mkdir -p "$plug"
install -m 755 "$target/release/nyx-agents-glimpse" "$plug/nyx-agents-glimpse"
install -m 644 "$src/apps/agents-glimpse/plugin.toml" "$plug/plugin.toml"

ext="${XDG_DATA_HOME:-$HOME/.local/share}/nyx-settings/extensions/agents"
mkdir -p "$ext"
install -m 644 "$src/data/settings/extension.toml" "$ext/"

if command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; then
  systemctl --user daemon-reload
  if [[ -z ${NYX_AGENTS_NO_ENABLE:-} ]]; then
    systemctl --user enable --now nyx-agentd.socket
    # A running manager picks up the new build; agents keep running.
    if systemctl --user is-active -q nyx-agentd.service; then
      systemctl --user restart nyx-agentd.service
    fi
  fi
fi
if ! command -v sway >/dev/null; then
  say "Agent workspaces need sway (headless, never on your screen): sudo pacman -S sway"
fi
say "Installed. Try: nyx-agents harnesses"
