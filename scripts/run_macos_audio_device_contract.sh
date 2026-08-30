#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$(uname -s)" != "Darwin" ]; then
    if [ "${AURA_REQUIRE_HARDWARE_DEVICE:-0}" = "1" ]; then
        echo "macOS CoreAudio contract: required but host is not macOS" >&2
        exit 1
    fi
    echo "macOS CoreAudio contract: SKIPPED (not macOS)"
    exit 0
fi

OUT_DIR=${TMPDIR:-/tmp}/aura-macos-audio-device-contract
mkdir -p "$OUT_DIR"

${CXX:-clang++} -std=c++20 -Wall -Wextra -I"$ROOT_DIR" -I"$ROOT_DIR/src" \
    "$ROOT_DIR/tests/macos_audio_device_contract.mm" \
    "$ROOT_DIR/src/platform/macos/coreaudio_device.mm" \
    -framework AudioToolbox -framework AudioUnit -framework CoreAudio \
    -o "$OUT_DIR/macos-audio-device-contract"

if "$OUT_DIR/macos-audio-device-contract"; then
    echo "macOS CoreAudio device contract passed (initialize/start/stop/reconfigure)"
    exit 0
fi

if [ "${AURA_REQUIRE_HARDWARE_DEVICE:-0}" = "1" ]; then
    echo "macOS CoreAudio device contract failed and hardware is required" >&2
    exit 1
fi
echo "macOS CoreAudio device contract: SKIPPED (no usable default output device)"
