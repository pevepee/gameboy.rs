#!/usr/bin/env bash
# Fetch the Blargg and mooneye test ROM suites into test-roms/.
#
# The ROMs are third-party binaries and are not committed. This script is
# idempotent: running it again updates the clones in place.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$root/test-roms"

mkdir -p "$dest"

clone() {
  local url="$1"
  local dir="$2"

  if [ -d "$dest/$dir/.git" ]; then
    echo "updating $dir"
    git -C "$dest/$dir" fetch --depth 1 origin
    git -C "$dest/$dir" reset --hard FETCH_HEAD
  else
    echo "cloning $dir"
    git clone --depth 1 "$url" "$dest/$dir"
  fi
}

clone https://github.com/retrio/gb-test-roms.git gb-test-roms
clone https://github.com/Gekkio/mooneye-test-suite.git mooneye-test-suite

# The harness walks test-roms/ recursively, so the suites can sit side by side.
count=$(find "$dest" -name '*.gb' | wc -l | tr -d ' ')
echo "ready: $count test ROMs under $dest"
