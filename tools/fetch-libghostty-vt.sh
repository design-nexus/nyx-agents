#!/usr/bin/env bash
# Download the pinned libghostty-vt release (source and Zig dependencies, about
# 10 MB) into vendor/libghostty-vt and check it against the committed SHA256SUMS,
# whose minisign signature is checked first when minisign is installed.
set -euo pipefail

VERSION=1.0.0
KEY=RWQlAjJC23149WL2sEpT/l0QKy7hMIFhYdQOFy0Z7z7PbneUgvlsnYcV
BASE="https://release.files.ghostty.org/libghostty-vt/$VERSION"

dir="$(cd "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/vendor/libghostty-vt"
cd "$dir"

if command -v minisign >/dev/null; then
  minisign -q -V -P "$KEY" -m SHA256SUMS
fi
for f in "libghostty-vt-$VERSION.tar.gz" "libghostty-vt-$VERSION-deps.tar.gz"; do
  if ! sha256sum --quiet --ignore-missing -c SHA256SUMS 2>/dev/null || [[ ! -f $f ]]; then
    curl -sfL --proto '=https' -o "$f.part" "$BASE/$f"
    mv "$f.part" "$f"
  fi
done
sha256sum --quiet --ignore-missing -c SHA256SUMS
echo "libghostty-vt $VERSION is in $dir"
