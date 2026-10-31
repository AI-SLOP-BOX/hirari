#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

# The fallback/generation matrix is Rust-owned
# (hirari-core-bridge/src/fallback_audio_device.rs). It replaced the C++
# `Hirari::Platform` device contract, which only this script and its test used.
cargo test --manifest-path "$ROOT_DIR/Cargo.toml" -p hirari-core-bridge --lib \
    fallback_audio_device

if [ "${HIRARI_REQUIRE_HARDWARE_DEVICE:-0}" = "1" ]; then
    if [ "$(uname -s)" != "Darwin" ]; then
        echo "HIRARI_REQUIRE_HARDWARE_DEVICE=1 requires macOS CoreAudio" >&2
        exit 1
    fi
    # The software matrix above validates fallback and generation behavior;
    # the strict mode must additionally exercise the real CoreAudio callback
    # instead of unconditionally failing before the hardware contract runs.
    HIRARI_REQUIRE_HARDWARE_DEVICE=1 \
        /bin/sh "$ROOT_DIR/scripts/run_macos_audio_device_contract.sh"
fi

echo "audio device transition contract passed (software fallback matrix)"
