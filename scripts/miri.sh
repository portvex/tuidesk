#!/bin/sh
# Run the unit tests under Miri, which detects undefined behaviour (invalid memory access,
# aliasing violations, data races, uninitialised reads) in tuidesk and the dependency code the
# tests exercise.
#   scripts/miri.sh              all unit tests that can run under Miri
#   scripts/miri.sh graphics     only tests whose name contains "graphics"
#
# Miri cannot call into C or the OS, so tests that start programs, use PTYs or sockets, or run
# Lua (a C library) are marked #[cfg_attr(miri, ignore)] and skipped. That includes tuidesk's own
# `unsafe` blocks, which are all such OS calls (see the SAFETY comments next to them).
set -e
cd "$(dirname "$0")/.."
# A Homebrew cargo/rustc earlier on PATH would shadow rustup's nightly.
PATH="$(dirname "$(rustup +nightly which rustc)"):$PATH"
export PATH

if ! cargo miri --version >/dev/null 2>&1; then
  echo "Installing Miri for the nightly toolchain..."
  rustup +nightly component add miri rust-src
fi

# Tests read temporary files and the clock, which Miri's isolation would refuse.
MIRIFLAGS="${MIRIFLAGS:--Zmiri-disable-isolation}"
export MIRIFLAGS
exec cargo miri test --lib -- --test-threads=4 "$@"
