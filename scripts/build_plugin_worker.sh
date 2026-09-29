#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if [ -n "${HIRARI_VST3_SDK:-}" ]; then
    exec "$ROOT_DIR/scripts/build_plugin_worker_with_vst3.sh"
fi

CXX_BIN=${CXX:-c++}
OUTPUT="${HIRARI_PLUGIN_WORKER_OUTPUT:-$ROOT_DIR/build-tools/hirari-plugin-host-worker}"
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT_DIR/target}
case "$TARGET_DIR" in /*) ;; *) TARGET_DIR="$ROOT_DIR/$TARGET_DIR" ;; esac

# The worker and in-process host share the same allocation-free Rust protocol
# implementation. Build it as a standalone static archive for this process.
cargo rustc --manifest-path "$ROOT_DIR/Cargo.toml" -p hirari-plugin-protocol \
    --release --features standalone-static -- --crate-type staticlib
PROTOCOL_LIB=$(find "$TARGET_DIR/release/deps" -maxdepth 1 -name 'libhirari_plugin_protocol-*.a' -print | head -n 1)
if [ ! -f "$PROTOCOL_LIB" ]; then
    echo "Rust plugin protocol archive was not produced: $PROTOCOL_LIB" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
LINK_FLAGS=""
case "$(uname -s)" in
    Darwin) LINK_FLAGS="-framework AudioToolbox -framework CoreFoundation" ;;
    *) LINK_FLAGS="-ldl" ;;
esac

# The worker is intentionally compiled as a standalone executable.  It is
# the only process allowed to load third-party plugin binaries.
$CXX_BIN -std=c++20 -O2 -Wall -Wextra -I"$ROOT_DIR" -I"$ROOT_DIR/src" "$ROOT_DIR/src/core/plugins/plugin_sandbox_worker.cpp" "$PROTOCOL_LIB" $LINK_FLAGS -o "$OUTPUT"
chmod 755 "$OUTPUT"
echo "Built: $OUTPUT"
