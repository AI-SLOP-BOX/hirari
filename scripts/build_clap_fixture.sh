#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

OUTPUT="${HIRARI_CLAP_FIXTURE_OUTPUT:-$ROOT_DIR/build-tools/minimal-gain.clap}"
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT_DIR/target}
case "$TARGET_DIR" in /*) ;; *) TARGET_DIR="$ROOT_DIR/$TARGET_DIR" ;; esac

# The fixture is a Rust cdylib that exports the single `clap_entry` symbol the
# isolated worker resolves with dlsym. It is a workspace member so cargo keeps
# checking its ABI layout assertions with the rest of the tree.
cargo build --manifest-path "$ROOT_DIR/Cargo.toml" -p hirari-minimal-gain-clap

case "$(uname -s)" in
    Darwin) BUILT="$TARGET_DIR/debug/libminimal_gain_clap.dylib" ;;
    *) BUILT="$TARGET_DIR/debug/libminimal_gain_clap.so" ;;
esac
if [ ! -f "$BUILT" ]; then
    echo "Rust CLAP fixture was not produced: $BUILT" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
cp "$BUILT" "$OUTPUT"
chmod 755 "$OUTPUT"
echo "Built: $OUTPUT"
