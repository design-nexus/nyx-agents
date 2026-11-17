#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
share=${XDG_DATA_HOME:-$HOME/.local/share}
config=${XDG_CONFIG_HOME:-$HOME/.config}
if [[ ${NYX_SKIP_BUILD:-0} != 1 ]]; then
 cargo build --release --locked --manifest-path "$root/Cargo.toml" -p nyx-ai-lens
fi
binary=${CARGO_TARGET_DIR:-$root/target}/release/nyx-ai-lens
install -Dm755 "$binary" "$HOME/.local/bin/nyx-ai-lens.new"
mv -f "$HOME/.local/bin/nyx-ai-lens.new" "$HOME/.local/bin/nyx-ai-lens"
mkdir -p "$share/nyx-ai-lens/qml"
cp -a "$root/apps/ai-lens/qml/." "$share/nyx-ai-lens/qml/"
install -Dm644 "$root/apps/ai-lens/data/io.github.design_nexus.nyx.AILens.desktop" "$share/applications/io.github.design_nexus.nyx.AILens.desktop"
install -Dm644 "$root/apps/ai-lens/data/io.github.design_nexus.nyx.AILens.svg" "$share/icons/hicolor/scalable/apps/io.github.design_nexus.nyx.AILens.svg"
# Shared Nyx runtime discovers this optional component; no NyxOS-only service required.
mkdir -p "$share/nyx-shell/components"
ln -sfn "$share/nyx-ai-lens/qml" "$share/nyx-shell/components/nyx-ai-lens"
# Omarchy loads its own host adapter; retain every existing plugin's files.
if [[ -d ${OMARCHY_PATH:-/usr/share/omarchy}/shell || -f $config/omarchy/shell.json ]]; then
 mkdir -p "$config/omarchy/plugins/design-nexus.ai-lens"
 cp -a "$root/apps/ai-lens/qml/." "$config/omarchy/plugins/design-nexus.ai-lens/"
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t -q "$share/icons/hicolor" || true
printf 'Installed AI Lens without opening or restarting it.\n'
