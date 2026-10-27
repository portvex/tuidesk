#!/bin/sh
# Run the fuzz targets with the nightly toolchain (cargo-fuzz needs it).
#   scripts/fuzz.sh                 every target for 60 seconds
#   scripts/fuzz.sh sixel 600       one target for 10 minutes
# Crashes are saved in fuzz/artifacts/<target>/; reproduce one with
#   scripts/fuzz.sh <target> 0 fuzz/artifacts/<target>/crash-...
set -e
cd "$(dirname "$0")/.."
# A Homebrew cargo/rustc earlier on PATH would shadow rustup's nightly.
PATH="$(dirname "$(rustup +nightly which rustc)"):$PATH"
export PATH

seconds=${2:-60}
if [ -n "$3" ]; then
  exec cargo fuzz run "$1" "$3"
fi
targets=${1:-$(cargo fuzz list)}
for t in $targets; do
  echo "== $t (${seconds}s)"
  cargo fuzz run "$t" -- -max_total_time="$seconds" -rss_limit_mb=4096
done
