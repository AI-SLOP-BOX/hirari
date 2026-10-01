#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

CXX=${CXX:-c++}
OUT_DIR=${TMPDIR:-/tmp}/hirari-tsan
mkdir -p "$OUT_DIR"

# The engine's C++ surface links against the Rust runtime. Build the staticlib
# first; without it every contract test below fails to link on undefined
# hirari_* symbols.
if ! cargo rustc -p hirari-core-bridge --lib --crate-type staticlib; then
    echo "failed to build the hirari-core-bridge staticlib" >&2
    exit 1
fi
BRIDGE_LIB=target/debug/libhirari_core_bridge.a
if [ ! -f "$BRIDGE_LIB" ]; then
    echo "hirari-core-bridge staticlib not found at $BRIDGE_LIB" >&2
    exit 1
fi

if ! "$CXX" -std=c++20 -fsanitize=thread -x c++ -o "$OUT_DIR/probe" - <<'EOF'
int main() { return 0; }
EOF
then
    echo "ThreadSanitizer is unavailable for $CXX" >&2
    exit 1
fi

COMMON_FLAGS="-std=c++20 -Wall -Wextra -I. -Isrc"
TSAN_FLAGS="-fsanitize=thread -fno-omit-frame-pointer"

"$CXX" $COMMON_FLAGS $TSAN_FLAGS \
    tests/realtime_safety_contract.cpp -o "$OUT_DIR/realtime-safety" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $TSAN_FLAGS \
    tests/graph_focus_waveform_contract.cpp -o "$OUT_DIR/graph-focus" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $TSAN_FLAGS \
    tests/effect_chain_watchdog_contract.cpp -o "$OUT_DIR/effect-chain-watchdog" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $TSAN_FLAGS \
    tests/block_processor_stft_contract.cpp -o "$OUT_DIR/block-processor-stft" "$BRIDGE_LIB"
"$CXX" $COMMON_FLAGS $TSAN_FLAGS \
    tests/rt_dsp_contract.cpp -o "$OUT_DIR/rt-dsp" "$BRIDGE_LIB"

TSAN_OPTIONS=${TSAN_OPTIONS:-halt_on_error=1:second_deadlock_stack=1}
export TSAN_OPTIONS
"$OUT_DIR/realtime-safety"
"$OUT_DIR/graph-focus"
"$OUT_DIR/effect-chain-watchdog"
echo "C++ ThreadSanitizer contract tests passed"
