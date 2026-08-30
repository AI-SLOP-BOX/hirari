#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SDK=${AURA_VST3_SDK:-/tmp/aura-vst3-sdk-new}
FIXTURE=${AURA_VST3_FIXTURE:-$HOME/Library/Audio/Plug-Ins/VST3/Surge XT.vst3}

if [ ! -f "$SDK/CMakeLists.txt" ] || [ ! -d "$SDK/public.sdk" ]; then
    echo "VST3 SDK is not prepared: $SDK" >&2
    echo "Set AURA_VST3_SDK to the official Steinberg VST3 SDK checkout." >&2
    exit 2
fi
if [ ! -e "$FIXTURE" ]; then
    echo "VST3 fixture is not available: $FIXTURE" >&2
    echo "Set AURA_VST3_FIXTURE to an installed SDK-compatible .vst3 bundle." >&2
    exit 2
fi

exec env \
    AURA_VST3_SDK="$SDK" \
    AURA_VST3_FIXTURE="$FIXTURE" \
    AURA_REQUIRE_VST3=1 \
    "$ROOT_DIR/scripts/run_vst3_e2e_smoke.sh"
