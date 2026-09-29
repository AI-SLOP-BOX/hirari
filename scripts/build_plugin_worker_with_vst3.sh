#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SDK_ROOT=${HIRARI_VST3_SDK:?Set HIRARI_VST3_SDK to the official Steinberg VST3 SDK root}
SDK_BUILD=${HIRARI_VST3_BUILD:-${TMPDIR:-/tmp}/hirari-vst3-sdk-build}
OUTPUT=${HIRARI_PLUGIN_WORKER_OUTPUT:-$ROOT_DIR/build-tools/hirari-plugin-host-worker}
CXX_BIN=${CXX:-c++}
ROOT_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT_DIR/target}
case "$ROOT_TARGET_DIR" in /*) ;; *) ROOT_TARGET_DIR="$ROOT_DIR/$ROOT_TARGET_DIR" ;; esac

cargo rustc --manifest-path "$ROOT_DIR/Cargo.toml" -p hirari-plugin-protocol \
    --release --features standalone-static -- --crate-type staticlib
PROTOCOL_LIB=$(find "$ROOT_TARGET_DIR/release/deps" -maxdepth 1 -name 'libhirari_plugin_protocol-*.a' -print | head -n 1)
if [ ! -f "$PROTOCOL_LIB" ]; then
    echo "Rust plugin protocol archive was not produced: $PROTOCOL_LIB" >&2
    exit 1
fi

if [ ! -f "$SDK_ROOT/CMakeLists.txt" ] || [ ! -d "$SDK_ROOT/public.sdk" ]; then
    echo "HIRARI_VST3_SDK is not a VST3 SDK checkout: $SDK_ROOT" >&2
    exit 2
fi

if [ ! -f "$SDK_BUILD/lib/libsdk_hosting.a" ]; then
    cmake -S "$SDK_ROOT" -B "$SDK_BUILD" \
        -DCMAKE_CXX_FLAGS=-DRELEASE \
        -DSMTG_ENABLE_VST3_HOSTING_EXAMPLES=OFF \
        -DSMTG_ENABLE_VST3_PLUGIN_EXAMPLES=OFF \
        -DSMTG_ENABLE_VSTGUI_SUPPORT=OFF \
        -DSMTG_CREATE_PLUGIN_LINK=OFF
    cmake --build "$SDK_BUILD" --target sdk_hosting -j2
fi

mkdir -p "$(dirname "$OUTPUT")"
$CXX_BIN -std=c++20 -O2 -Wall -Wextra -Wno-deprecated-declarations \
    -DHIRARI_ENABLE_VST3_SDK -fobjc-arc \
    -I"$ROOT_DIR" -I"$ROOT_DIR/src" -I"$SDK_ROOT" \
    "$ROOT_DIR/src/core/plugins/plugin_sandbox_worker.cpp" \
    "$SDK_ROOT/public.sdk/source/vst/hosting/plugprovider.cpp" \
    "$SDK_ROOT/public.sdk/source/vst/hosting/eventlist.cpp" \
    "$SDK_ROOT/public.sdk/source/vst/hosting/module_mac.mm" \
    "$SDK_ROOT/public.sdk/source/common/memorystream.cpp" \
    "$PROTOCOL_LIB" \
    -L"$SDK_BUILD/lib" -lsdk_hosting -lsdk_common -lbase -lpluginterfaces \
    -framework AudioToolbox -framework CoreFoundation -framework Cocoa \
    -o "$OUTPUT"
chmod 755 "$OUTPUT"
echo "Built VST3-enabled worker: $OUTPUT"
