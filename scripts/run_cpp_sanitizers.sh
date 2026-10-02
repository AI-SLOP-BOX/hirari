#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

CXX=${CXX:-c++}
OUT_DIR=${TMPDIR:-/tmp}/hirari-sanitizers
mkdir -p "$OUT_DIR"

if ! command -v "$CXX" >/dev/null 2>&1; then
    echo "C++ compiler not found: $CXX" >&2
    exit 1
fi

# The engine's C++ surface links against the Rust runtime. Build the staticlib
# first; without it every contract test below fails to link on undefined
# hirari_* symbols.
BRIDGE_LIB=$("$ROOT_DIR/scripts/build_bridge_staticlib.sh")

SANITIZER_FLAGS="-fsanitize=address,undefined -fno-omit-frame-pointer"
COMMON_FLAGS="-std=c++20 -Wall -Wextra -I. -Isrc"

"$CXX" $COMMON_FLAGS $SANITIZER_FLAGS \
    tests/pdc_dry_delay_contract.cpp -o "$OUT_DIR/pdc-dry-delay" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $SANITIZER_FLAGS \
    tests/graph_focus_waveform_contract.cpp -o "$OUT_DIR/graph-focus" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $SANITIZER_FLAGS \
    tests/effect_chain_watchdog_contract.cpp -o "$OUT_DIR/effect-chain-watchdog" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $SANITIZER_FLAGS \
    tests/waveform_generation_contract.cpp -o "$OUT_DIR/waveform-generation" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $SANITIZER_FLAGS \
    tests/block_processor_stft_contract.cpp -o "$OUT_DIR/block-processor-stft" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $SANITIZER_FLAGS \
    tests/rt_dsp_contract.cpp -o "$OUT_DIR/rt-dsp" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $SANITIZER_FLAGS \
    tests/wav_roundtrip_contract.cpp -o "$OUT_DIR/wav-roundtrip" "$BRIDGE_LIB"

if [ "$(uname -s)" = "Darwin" ]; then
    DEFAULT_ASAN_OPTIONS='detect_leaks=0:halt_on_error=1'
else
    DEFAULT_ASAN_OPTIONS='detect_leaks=1:halt_on_error=1'
fi
ASAN_OPTIONS=${ASAN_OPTIONS:-$DEFAULT_ASAN_OPTIONS}
UBSAN_OPTIONS=${UBSAN_OPTIONS:-halt_on_error=1:print_stacktrace=1}
export ASAN_OPTIONS UBSAN_OPTIONS

"$OUT_DIR/pdc-dry-delay"
"$OUT_DIR/graph-focus"
"$OUT_DIR/effect-chain-watchdog"
"$OUT_DIR/waveform-generation"
"$OUT_DIR/block-processor-stft"
"$OUT_DIR/rt-dsp"
"$OUT_DIR/wav-roundtrip"
echo "C++ ASan/UBSan contract tests passed"
