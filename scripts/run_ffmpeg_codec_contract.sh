#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${TMPDIR:-/tmp}/aura-ffmpeg-engine-contract

if [ "$(uname -s)" = "Darwin" ]; then
    CXX=${CXX:-c++}
else
    CXX=${CXX:-c++}
fi

"$CXX" -std=c++20 -Wall -Wextra -I"$ROOT_DIR" -I"$ROOT_DIR/src" \
    "$ROOT_DIR/tests/ffmpeg_engine_contract.cpp" -o "$OUT"
"$OUT"
rm -f "$OUT"
echo "FFmpeg codec contract passed"
