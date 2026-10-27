#!/bin/sh
# Code coverage of all tests (unit, tmux and image tests, including the tuidesk servers they start).
#   scripts/coverage.sh            per-file summary and an HTML report (target/llvm-cov/html)
#   scripts/coverage.sh --check    fail if coverage is below the floors below (pre-push hook)
#   scripts/coverage.sh --lcov     write target/lcov.info for other tools
set -e
cd "$(dirname "$0")/.."

# Floors for --check, a little under the current numbers so coverage can only go up.
# Raise them when tests improve.
MIN_LINES=77
MIN_FUNCTIONS=79

# cargo-llvm-cov needs rustup's llvm-tools; a Homebrew cargo/rustc earlier on PATH would
# shadow rustup's toolchain.
PATH="$(dirname "$(rustup +stable which rustc)"):$PATH"
export PATH
if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "cargo-llvm-cov is missing: cargo install cargo-llvm-cov" >&2
  exit 1
fi
rustup +stable component add llvm-tools-preview >/dev/null 2>&1 || true

case "$1" in
  --check)
    cargo llvm-cov --all-targets --summary-only \
      --fail-under-lines "$MIN_LINES" --fail-under-functions "$MIN_FUNCTIONS"
    ;;
  --lcov)
    cargo llvm-cov --all-targets --lcov --output-path target/lcov.info
    echo "wrote target/lcov.info"
    ;;
  "")
    cargo llvm-cov --all-targets --html
    cargo llvm-cov report --summary-only
    echo "HTML report: target/llvm-cov/html/index.html"
    ;;
  *)
    echo "usage: $0 [--check | --lcov]" >&2
    exit 2
    ;;
esac
