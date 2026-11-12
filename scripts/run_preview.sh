#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT_DIR/target/release/hirari-ui"

if [ ! -x "$BIN" ]; then
    echo "Hirari preview binary is missing. Run: cargo build --release -p hirari-ui" >&2
    exit 1
fi

exec "$BIN" "$@"
