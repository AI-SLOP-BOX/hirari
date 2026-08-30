#!/bin/sh
set -eu
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUTPUT_DIR="$ROOT_DIR/build-tools"
OUTPUT="$OUTPUT_DIR/pdc-graph-smoke"
mkdir -p "$OUTPUT_DIR"
${CXX:-clang++} -std=c++20 -I"$ROOT_DIR" \
  "$ROOT_DIR/tests/pdc_graph_smoke.cpp" -o "$OUTPUT"
"$OUTPUT"
echo "pdc graph smoke passed"
