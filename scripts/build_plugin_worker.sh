#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if [ -n "${AURA_VST3_SDK:-}" ]; then
    exec "$ROOT_DIR/scripts/build_plugin_worker_with_vst3.sh"
fi

CXX_BIN=${CXX:-c++}
OUTPUT="${AURA_PLUGIN_WORKER_OUTPUT:-$ROOT_DIR/build-tools/aura-plugin-host-worker}"

mkdir -p "$(dirname "$OUTPUT")"
LINK_FLAGS=""
case "$(uname -s)" in
    Darwin) LINK_FLAGS="-framework AudioToolbox -framework CoreFoundation" ;;
    *) LINK_FLAGS="-ldl" ;;
esac

# The worker is intentionally compiled as a standalone executable.  It is
# the only process allowed to load third-party plugin binaries.
$CXX_BIN -std=c++20 -O2 -Wall -Wextra -I"$ROOT_DIR" -I"$ROOT_DIR/src" "$ROOT_DIR/src/core/plugins/plugin_sandbox_worker.cpp" $LINK_FLAGS -o "$OUTPUT"
chmod 755 "$OUTPUT"
echo "Built: $OUTPUT"
