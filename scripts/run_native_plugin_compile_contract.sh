#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

# Compile the real out-of-process worker as part of the normal native
# contract.  The header-only contract below cannot catch translation-unit
# errors in the executable that loads third-party plugins.
"$ROOT_DIR/scripts/build_plugin_worker.sh"

# The worker is an external-process trust boundary. Invalid option values and
# unknown options must fail before any fd, shared-memory, or plugin setup.
if "$ROOT_DIR/build-tools/aura-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd not-a-number; then
    echo "worker accepted a malformed file descriptor" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/aura-plugin-host-worker" \
    --plugin builtin://passthrough --unknown-option value; then
    echo "worker accepted an unknown option" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/aura-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd 0 --status-fd 1 --shared-fd 2 \
    --sample-rate nan --min-frames 1 --max-frames 64 --channels 2; then
    echo "worker accepted a non-finite sample rate" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/aura-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd 0 --status-fd 1 --shared-fd 2 \
    --sample-rate 48000 --min-frames 0 --max-frames 64 --channels 2; then
    echo "worker accepted a zero minimum frame count" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/aura-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd 0 --status-fd 1 --shared-fd 2 \
    --sample-rate 48000 --min-frames 128 --max-frames 64 --channels 2; then
    echo "worker accepted an inverted frame range" >&2
    exit 1
fi

CXX=${CXX:-c++}
OUT_DIR=${TMPDIR:-/tmp}/aura-native-contract
mkdir -p "$OUT_DIR"

LINK_FLAGS=""
if [ "$(uname -s)" = "Darwin" ]; then
    LINK_FLAGS="-framework AudioToolbox -framework CoreAudio"
fi

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/native_plugin_compile_contract.cpp src/io/coreaudio_driver.cpp \
    src/core/engine/midi_orchestrator.cpp $LINK_FLAGS \
    -o "$OUT_DIR/native-plugin-compile"
"$OUT_DIR/native-plugin-compile"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/wav_io_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/wav-io-contract"
"$OUT_DIR/wav-io-contract"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/project_decoder_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/project-decoder-contract"
"$OUT_DIR/project-decoder-contract"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/persistence_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/persistence-contract"
"$OUT_DIR/persistence-contract"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/timeline_track_render_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/timeline-track-render-contract"
"$OUT_DIR/timeline-track-render-contract"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/recording_engine_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/recording-engine-contract"
"$OUT_DIR/recording-engine-contract"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/recording_engine_stress.cpp $LINK_FLAGS \
    -o "$OUT_DIR/recording-engine-stress"
AURA_RECORDING_STRESS_ROUNDS="${AURA_RECORDING_STRESS_ROUNDS:-250}" \
    "$OUT_DIR/recording-engine-stress"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/recording_autosave_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/recording-autosave-contract"
"$OUT_DIR/recording-autosave-contract"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/shared_memory_multiprocess_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/shared-memory-multiprocess-contract"
"$OUT_DIR/shared-memory-multiprocess-contract"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/sidechain_snapshot_contract.cpp $LINK_FLAGS \
    -o "$OUT_DIR/sidechain-snapshot-contract"
"$OUT_DIR/sidechain-snapshot-contract"

"$ROOT_DIR/scripts/build_clap_fixture.sh" >/dev/null
"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/clap_direct_process_contract.cpp \
    -ldl $LINK_FLAGS -o "$OUT_DIR/clap-direct-process-contract"
"$OUT_DIR/clap-direct-process-contract" "$ROOT_DIR/build-tools/minimal-gain.clap"

"$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/aura_omni_contract.cpp \
    -o "$OUT_DIR/aura-omni-contract"
"$OUT_DIR/aura-omni-contract"
echo "native plugin compile contract passed"
