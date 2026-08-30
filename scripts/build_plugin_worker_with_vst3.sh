#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SDK_ROOT=${AURA_VST3_SDK:?Set AURA_VST3_SDK to the official Steinberg VST3 SDK root}
SDK_BUILD=${AURA_VST3_BUILD:-${TMPDIR:-/tmp}/aura-vst3-sdk-build}
OUTPUT=${AURA_PLUGIN_WORKER_OUTPUT:-$ROOT_DIR/build-tools/aura-plugin-host-worker}
CXX_BIN=${CXX:-c++}

if [ ! -f "$SDK_ROOT/CMakeLists.txt" ] || [ ! -d "$SDK_ROOT/public.sdk" ]; then
    echo "AURA_VST3_SDK is not a VST3 SDK checkout: $SDK_ROOT" >&2
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
    -DAURA_ENABLE_VST3_SDK -fobjc-arc \
    -I"$ROOT_DIR" -I"$ROOT_DIR/src" -I"$SDK_ROOT" \
    "$ROOT_DIR/src/core/plugins/plugin_sandbox_worker.cpp" \
    "$SDK_ROOT/public.sdk/source/vst/hosting/plugprovider.cpp" \
    "$SDK_ROOT/public.sdk/source/vst/hosting/eventlist.cpp" \
    "$SDK_ROOT/public.sdk/source/vst/hosting/module_mac.mm" \
    "$SDK_ROOT/public.sdk/source/common/memorystream.cpp" \
    -L"$SDK_BUILD/lib" -lsdk_hosting -lsdk_common -lbase -lpluginterfaces \
    -framework AudioToolbox -framework CoreFoundation -framework Cocoa \
    -o "$OUTPUT"
chmod 755 "$OUTPUT"
echo "Built VST3-enabled worker: $OUTPUT"
