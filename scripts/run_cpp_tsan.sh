#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

CXX=${CXX:-c++}
OUT_DIR=${TMPDIR:-/tmp}/aura-tsan
mkdir -p "$OUT_DIR"

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
    tests/realtime_safety_contract.cpp -o "$OUT_DIR/realtime-safety"
"$CXX" $COMMON_FLAGS $TSAN_FLAGS \
    tests/graph_focus_waveform_contract.cpp -o "$OUT_DIR/graph-focus"
"$CXX" $COMMON_FLAGS $TSAN_FLAGS \
    tests/effect_chain_watchdog_contract.cpp -o "$OUT_DIR/effect-chain-watchdog"

TSAN_OPTIONS=${TSAN_OPTIONS:-halt_on_error=1:second_deadlock_stack=1}
export TSAN_OPTIONS
"$OUT_DIR/realtime-safety"
"$OUT_DIR/graph-focus"
"$OUT_DIR/effect-chain-watchdog"
echo "C++ ThreadSanitizer contract tests passed"
