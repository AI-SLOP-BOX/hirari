#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SDK=${HIRARI_VST3_SDK:-/tmp/hirari-vst3-sdk-new}
FIXTURE=${HIRARI_VST3_FIXTURE:-$HOME/Library/Audio/Plug-Ins/VST3/Surge XT.vst3}

if [ ! -f "$SDK/CMakeLists.txt" ] || [ ! -d "$SDK/public.sdk" ]; then
    echo "VST3 SDK is not prepared: $SDK" >&2
    echo "Set HIRARI_VST3_SDK to the official Steinberg VST3 SDK checkout." >&2
    exit 2
fi
if [ ! -e "$FIXTURE" ]; then
    echo "VST3 fixture is not available: $FIXTURE" >&2
    echo "Set HIRARI_VST3_FIXTURE to an installed SDK-compatible .vst3 bundle." >&2
    exit 2
fi

exec env \
    HIRARI_VST3_SDK="$SDK" \
    HIRARI_VST3_FIXTURE="$FIXTURE" \
    HIRARI_REQUIRE_VST3=1 \
    "$ROOT_DIR/scripts/run_vst3_e2e_smoke.sh"
