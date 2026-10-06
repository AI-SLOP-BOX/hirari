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
if "$ROOT_DIR/build-tools/hirari-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd not-a-number; then
    echo "worker accepted a malformed file descriptor" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/hirari-plugin-host-worker" \
    --plugin builtin://passthrough --unknown-option value; then
    echo "worker accepted an unknown option" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/hirari-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd 0 --status-fd 1 --shared-fd 2 \
    --sample-rate nan --min-frames 1 --max-frames 64 --channels 2; then
    echo "worker accepted a non-finite sample rate" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/hirari-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd 0 --status-fd 1 --shared-fd 2 \
    --sample-rate 48000 --min-frames 0 --max-frames 64 --channels 2; then
    echo "worker accepted a zero minimum frame count" >&2
    exit 1
fi
if "$ROOT_DIR/build-tools/hirari-plugin-host-worker" \
    --plugin builtin://passthrough --control-fd 0 --status-fd 1 --shared-fd 2 \
    --sample-rate 48000 --min-frames 128 --max-frames 64 --channels 2; then
    echo "worker accepted an inverted frame range" >&2
    exit 1
fi

CXX=${CXX:-c++}
OUT_DIR=${TMPDIR:-/tmp}/hirari-native-contract
mkdir -p "$OUT_DIR"

# The native surface is backed by the Rust core bridge. Without the staticlib,
# every hirari_* symbol in the contract below is undefined at link time.
BRIDGE_LIB=$("$ROOT_DIR/scripts/build_bridge_staticlib.sh")

# Shared link inputs for every contract on macOS. The engine's CoreAudio
# driver translation unit references AudioComponent/AudioUnit/AudioObject
# symbols, and the Rust staticlib archives Objective-C driver objects; with
# LINK_FLAGS left empty those symbols stayed unresolved, so
# native-plugin-compile could never leave BUILD_OR_LINK_FAILED. This block
# existed earlier in the script's history and was lost in a later refactor.
# HIRARI_NO_FRAMEWORKS=1 disables it for negative-control runs.
LINK_FLAGS=""
if [ "$(uname -s)" = "Darwin" ] && [ "${HIRARI_NO_FRAMEWORKS:-0}" != "1" ]; then
    LINK_FLAGS="-framework Foundation -framework CoreAudio -framework CoreMIDI -framework AudioToolbox -framework CoreMedia"
fi

# ---------------------------------------------------------------------------
# Contract runner
#
# Each contract is independent, so one failure must not hide the ones behind
# it. Build, link and run every contract, recording each outcome, then report a
# full accounting and exit non-zero if any of them failed. The previous
# fail-fast structure meant a single defect masked every later contract, which
# is how a mono-export bug and a recording-publish bug both stayed latent.
# ---------------------------------------------------------------------------
CONTRACT_NAMES=""
CONTRACT_STATUS=""

# Extra runtime arguments for the contract binary. Link inputs stay in "$@" so
# the linker sees them; a contract whose main() reads argv[1..] sets this
# immediately before calling build_link_run. It is consumed on entry, so a
# value can never leak into a later contract during the same run.
CONTRACT_RUN_ARGS=""

record() {
    CONTRACT_NAMES="$CONTRACT_NAMES $1"
    CONTRACT_STATUS="$CONTRACT_STATUS $2"
}

build_link_run() {
    # $1 = label, $2 = source path, rest = extra link inputs.
    # Runtime arguments come from CONTRACT_RUN_ARGS, never from "$@": the
    # linker must not see them, and the runner must not see link inputs.
    label=$1
    source=$2
    shift 2
    run_args=${CONTRACT_RUN_ARGS:-}
    CONTRACT_RUN_ARGS=""
    binary="$OUT_DIR/$label"
    # A failed build must never be able to execute a binary left behind by an
    # earlier successful one, so any previous artifact goes first.
    rm -f "$binary"
    # Linking the large staticlib is memory-hungry and this machine can OOM the
    # compiler without reporting a diagnostic. Retry once before declaring the
    # link failed, so an environment kill is not mistaken for a real defect.
    linked=1
    attempt=1
    while [ "$attempt" -le 2 ]; do
        if "$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
            "$source" "$@" $LINK_FLAGS -o "$binary"; then
            linked=0
            break
        fi
        attempt=$((attempt + 1))
    done
    if [ "$linked" -ne 0 ]; then
        record "$label" BUILD_OR_LINK_FAILED
        return 0
    fi
    if [ "$(uname -s)" = "Darwin" ]; then
        if (ulimit -s 16384 && "$binary" $run_args); then
            record "$label" PASSED
        else
            record "$label" FAILED
        fi
    elif "$binary" $run_args; then
        record "$label" PASSED
    else
        record "$label" FAILED
    fi
}

# The main contract needs the audio driver translation unit and the frameworks.
# Same memory-driven retry as build_link_run: this translation unit pulls in
# the whole engine and is the most likely to be OOM-killed on a small machine.
main_linked=1
main_attempt=1
while [ "$main_attempt" -le 2 ]; do
    if "$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
        tests/native_plugin_compile_contract.cpp src/io/coreaudio_driver.cpp \
        "$BRIDGE_LIB" $LINK_FLAGS -o "$OUT_DIR/native-plugin-compile"; then
        main_linked=0
        break
    fi
    main_attempt=$((main_attempt + 1))
done
if [ "$main_linked" -ne 0 ]; then
    record native-plugin-compile BUILD_OR_LINK_FAILED
else
    # This translation unit intentionally exercises the complete native surface
    # in one process. Its aggregate RAII locals are larger than macOS's default
    # 8-MiB main-thread stack; raise only the child process limit.
    if [ "$(uname -s)" = "Darwin" ]; then
        if (ulimit -s 16384 && "$OUT_DIR/native-plugin-compile"); then
            record native-plugin-compile PASSED
        else
            record native-plugin-compile FAILED
        fi
    elif "$OUT_DIR/native-plugin-compile"; then
        record native-plugin-compile PASSED
    else
        record native-plugin-compile FAILED
    fi
fi

build_link_run wav-io-contract tests/wav_io_contract.cpp "$BRIDGE_LIB"
build_link_run project-decoder-contract tests/project_decoder_contract.cpp "$BRIDGE_LIB"
build_link_run persistence-contract tests/persistence_contract.cpp "$BRIDGE_LIB"
build_link_run timeline-track-render-contract tests/timeline_track_render_contract.cpp "$BRIDGE_LIB"
# The BPM ghost contract pulls the engine (and therefore the Apple driver
# frameworks) in through the shared LINK_FLAGS above.
build_link_run timeline-bpm-ghost-contract tests/timeline_bpm_ghost_contract.cpp \
    "$BRIDGE_LIB"
build_link_run recording-engine-contract tests/recording_engine_contract.cpp "$BRIDGE_LIB"
build_link_run recording-autosave-contract tests/recording_autosave_contract.cpp "$BRIDGE_LIB"
build_link_run shared-memory-multiprocess-contract tests/shared_memory_multiprocess_contract.cpp "$BRIDGE_LIB"
build_link_run sidechain-snapshot-contract tests/sidechain_snapshot_contract.cpp "$BRIDGE_LIB"
build_link_run vocal-restoration-contract tests/vocal_restoration_contract.cpp "$BRIDGE_LIB"
build_link_run cli-media-processor-contract tests/cli_media_processor_contract.cpp
build_link_run spectral-editor-contract tests/spectral_editor_contract.cpp "$BRIDGE_LIB"
build_link_run sampler-engine-contract tests/sampler_engine_contract.cpp "$BRIDGE_LIB"
build_link_run audio-buffer-contract tests/audio_buffer_contract.cpp "$BRIDGE_LIB"
build_link_run mmap-audio-contract tests/mmap_audio_contract.cpp "$BRIDGE_LIB"
build_link_run range-edit-contract tests/range_edit_contract.cpp "$BRIDGE_LIB"

# The CLAP direct-process contract's main() takes exactly one runtime argument
# (the fixture path), and its objects reference hirari_* buffer symbols while
# linking, so the staticlib stays a link input but only the fixture is a run
# argument.
"$ROOT_DIR/scripts/build_clap_fixture.sh" >/dev/null
CONTRACT_RUN_ARGS="$ROOT_DIR/build-tools/minimal-gain.clap"
build_link_run clap-direct-process-contract \
    tests/clap_direct_process_contract.cpp -ldl "$BRIDGE_LIB"
build_link_run hirari-omni-contract tests/hirari_omni_contract.cpp

# The stress contract is long and round-tunable, so it runs only when asked,
# but it is still built so a link break is reported rather than hidden.
if ! "$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Isrc/external \
    tests/recording_engine_stress.cpp "$BRIDGE_LIB" $LINK_FLAGS \
    -o "$OUT_DIR/recording-engine-stress"; then
    record recording-engine-stress BUILD_OR_LINK_FAILED
elif [ "${HIRARI_RUN_RECORDING_STRESS:-0}" = "1" ]; then
    HIRARI_RECORDING_STRESS_ROUNDS="${HIRARI_RECORDING_STRESS_ROUNDS:-250}" \
        "$OUT_DIR/recording-engine-stress"
    record recording-engine-stress PASSED
else
    # Linked, but the run is opt-in; record it as linked-only so the summary
    # does not claim an execution that never happened.
    record recording-engine-stress NOT_RUN_OPTIONAL
fi

echo "native contract summary:"
overall=0
# Report each name with its status; statuses are recorded in the same order.
set -- $CONTRACT_STATUS
for name in $CONTRACT_NAMES; do
    status=$1
    shift
    printf '  %-34s %s\n' "$name" "$status"
    if [ "$status" != "PASSED" ] && [ "$status" != "NOT_RUN_OPTIONAL" ]; then
        overall=1
    fi
done
if [ "$overall" -ne 0 ]; then
    echo "native plugin compile contract FAILED" >&2
    exit 1
fi
echo "native plugin compile contract passed"
