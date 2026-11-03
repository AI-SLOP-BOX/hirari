#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$(uname -s)" != "Darwin" ]; then
    if [ "${HIRARI_REQUIRE_HARDWARE_DEVICE:-0}" = "1" ]; then
        echo "macOS CoreAudio contract: required but host is not macOS" >&2
        exit 1
    fi
    echo "macOS CoreAudio contract: SKIPPED (not macOS)"
    exit 0
fi

# The CoreAudio device lifecycle is Rust-owned (`mac_audio_driver.rs`), so the
# contract runs against `MacAudioHost` directly instead of through a C++ adapter
# that only forwarded across the C ABI. The test is `#[ignore]`d because CI hosts
# have no usable default output device; this script is the hardware path.
TEST=mac_audio_driver::tests::real_device_starts_renegotiates_and_stops
if (cd "$ROOT_DIR" && cargo test -p hirari-core-bridge --lib -- --exact --ignored "$TEST"); then
    echo "macOS CoreAudio device contract passed (initialize/start/stop/reconfigure)"
    exit 0
fi

if [ "${HIRARI_REQUIRE_HARDWARE_DEVICE:-0}" = "1" ]; then
    echo "macOS CoreAudio device contract failed and hardware is required" >&2
    exit 1
fi
echo "macOS CoreAudio device contract: SKIPPED (no usable default output device)"
