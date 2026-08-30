#!/bin/sh
set -eu

# Reproducible clean-check gate. The target directory is disposable and is
# never placed inside the repository, so a stale local build cannot satisfy
# this check accidentally.
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CLEAN_TARGET=${AURA_CLEAN_TARGET_DIR:-${TMPDIR:-/tmp}/aura-clean-target.$$.${RANDOM:-0}}
KEEP=${AURA_KEEP_CLEAN_TARGET:-0}
cleanup() {
  [ "$KEEP" = 1 ] || rm -rf "$CLEAN_TARGET"
}
trap cleanup EXIT INT TERM
mkdir -p "$CLEAN_TARGET"
cd "$ROOT_DIR"

# A clean macOS UI build can temporarily require several GiB. Fail before
# compilation when the filesystem cannot hold a meaningful result.
AVAILABLE_KIB=$(df -Pk "$CLEAN_TARGET" | awk 'NR==2 {print $4}')
if [ "${AVAILABLE_KIB:-0}" -lt 8388608 ]; then
  echo "clean build requires at least 8 GiB free at $CLEAN_TARGET" >&2
  exit 1
fi

export CARGO_TARGET_DIR="$CLEAN_TARGET"
export CARGO_INCREMENTAL=0
cargo fetch --locked
cargo check --workspace --locked
cargo test --workspace --all-targets --locked -- --test-threads=1
printf 'clean build verification passed: %s\n' "$CLEAN_TARGET"
