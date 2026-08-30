#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CXX_BIN=${CXX:-c++}
OUTPUT=${AURA_CLAP_FIXTURE_OUTPUT:-$ROOT_DIR/build-tools/minimal-gain.clap}
mkdir -p "$(dirname "$OUTPUT")"

case "$(uname -s)" in
    Darwin) "$CXX_BIN" -std=c++20 -dynamiclib -O2 -Wall -Wextra \
        -I"$ROOT_DIR" -I"$ROOT_DIR/src" \
        "$ROOT_DIR/tests/fixtures/minimal_gain_clap.cpp" -o "$OUTPUT" ;;
    *) "$CXX_BIN" -std=c++20 -fPIC -shared -O2 -Wall -Wextra \
        -I"$ROOT_DIR" -I"$ROOT_DIR/src" \
        "$ROOT_DIR/tests/fixtures/minimal_gain_clap.cpp" -o "$OUTPUT" ;;
esac
chmod 755 "$OUTPUT"
echo "Built: $OUTPUT"
