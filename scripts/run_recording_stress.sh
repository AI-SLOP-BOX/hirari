#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

CXX=${CXX:-c++}
OUT_DIR=${TMPDIR:-/tmp}/aura-recording-stress
mkdir -p "$OUT_DIR"

LINK_FLAGS=""
if [ "$(uname -s)" = "Darwin" ]; then
    LINK_FLAGS="-framework AudioToolbox -framework CoreAudio"
fi

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/recording_engine_stress.cpp $LINK_FLAGS \
    -o "$OUT_DIR/recording-engine-stress"

AURA_RECORDING_STRESS_DURATION_SECONDS="${AURA_RECORDING_STRESS_DURATION_SECONDS:-60}" \
AURA_RECORDING_STRESS_ROUNDS="${AURA_RECORDING_STRESS_ROUNDS:-1000000}" \
AURA_RECORDING_STRESS_BLOCKS_PER_TAKE="${AURA_RECORDING_STRESS_BLOCKS_PER_TAKE:-256}" \
    "$OUT_DIR/recording-engine-stress"

echo "recording stress passed"
