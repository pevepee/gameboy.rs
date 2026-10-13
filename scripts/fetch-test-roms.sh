#!/usr/bin/env bash
# Fetch the test ROM suites into test-roms/.
#
# The ROMs are third-party binaries and are not committed. This script is
# idempotent: running it again updates the clones in place.
#
# The Blargg suite is required — the harness asserts on it when
# GB_REQUIRE_TEST_ROMS=1. The mooneye suite is optional, so a move or rename
# upstream downgrades to a warning instead of failing the whole job.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$root/test-roms"

mkdir -p "$dest"

# Clone or fast-forward a suite. Returns non-zero on any git failure.
fetch() {
  local name="$1"
  local url="$2"
  local dir="$dest/$name"

  if [ -d "$dir/.git" ]; then
    echo "updating $name"
    git -C "$dir" fetch --depth 1 origin
    git -C "$dir" reset --hard FETCH_HEAD
  else
    echo "cloning $name from $url"
    git clone --depth 1 "$url" "$dir"
  fi
}

if ! fetch gb-test-roms https://github.com/retrio/gb-test-roms.git; then
  echo "error: could not fetch the Blargg suite (retrio/gb-test-roms)" >&2
  echo "error: the CI job must fail here rather than report a test result" >&2
  exit 1
fi

if ! fetch mooneye-test-suite https://github.com/Gekkio/mooneye-test-suite.git; then
  echo "warning: mooneye suite unavailable; continuing without it" >&2
fi

# The harness walks test-roms/ recursively, so the suites sit side by side.
count=$(find "$dest" -name '*.gb' -type f | wc -l | tr -d ' ')
echo "ready: $count test ROMs under $dest"

if [ "$count" -eq 0 ]; then
  echo "error: no .gb files found; check that the clones succeeded" >&2
  exit 1
fi
