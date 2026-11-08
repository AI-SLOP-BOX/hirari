#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

OUTPUT="${HIRARI_PLUGIN_WORKER_OUTPUT:-$ROOT_DIR/build-tools/hirari-plugin-host-worker}"
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT_DIR/target}
case "$TARGET_DIR" in /*) ;; *) TARGET_DIR="$ROOT_DIR/$TARGET_DIR" ;; esac

# The worker is a Rust executable: the mailbox layout, argument parsing, process
# limits, mapping, adapter selection, the control loop and teardown all live in
# hirari-plugin-protocol, and the macOS AudioUnit surface links its own
# frameworks through the crate's `#[link]` attributes.
# Set HIRARI_VST3_SDK to an official Steinberg VST3 SDK checkout to compile the
# VST3 adapter into the worker; without it, VST3 requests fail closed.
cargo build --manifest-path "$ROOT_DIR/Cargo.toml" -p hirari-plugin-protocol \
    --release --features worker-runtime --bin hirari-plugin-host-worker
BUILT="$TARGET_DIR/release/hirari-plugin-host-worker"
if [ ! -f "$BUILT" ]; then
    echo "Rust worker executable was not produced: $BUILT" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
cp "$BUILT" "$OUTPUT"
chmod 755 "$OUTPUT"
echo "Built: $OUTPUT"
