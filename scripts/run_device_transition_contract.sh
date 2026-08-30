#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${TMPDIR:-/tmp}/aura-device-transition-contract
mkdir -p "$OUT_DIR"

${CXX:-c++} -std=c++20 -Wall -Wextra -I"$ROOT_DIR" -I"$ROOT_DIR/src" \
    "$ROOT_DIR/tests/audio_device_transition_contract.cpp" \
    -o "$OUT_DIR/audio-device-transition"
"$OUT_DIR/audio-device-transition"

if [ "${AURA_REQUIRE_HARDWARE_DEVICE:-0}" = "1" ]; then
    if [ "$(uname -s)" != "Darwin" ]; then
        echo "AURA_REQUIRE_HARDWARE_DEVICE=1 requires macOS CoreAudio" >&2
        exit 1
    fi
    # The software matrix above validates fallback and generation behavior;
    # the strict mode must additionally exercise the real CoreAudio callback
    # instead of unconditionally failing before the hardware contract runs.
    AURA_REQUIRE_HARDWARE_DEVICE=1 \
        /bin/sh "$ROOT_DIR/scripts/run_macos_audio_device_contract.sh"
fi

echo "audio device transition contract passed (software fallback matrix)"
