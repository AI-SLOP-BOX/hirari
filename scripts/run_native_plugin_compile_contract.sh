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
# Each check is independent, so one failure must not hide the ones behind it.
# Build, link and run every contract, recording each outcome, then report a
# full accounting and exit non-zero if any of them failed. The previous
# fail-fast structure meant a single defect masked every later contract, which
# is how a mono-export bug and a recording-publish bug both stayed latent.
# ---------------------------------------------------------------------------
CONTRACT_NAMES=""
CONTRACT_STATUS=""

record() {
    CONTRACT_NAMES="$CONTRACT_NAMES $1"
    CONTRACT_STATUS="$CONTRACT_STATUS $2"
}

# The main contract needs the audio driver translation unit and the frameworks.
# Linking it pulls in the whole engine and the Rust staticlib, which is
# memory-hungry enough that this machine can OOM the compiler without reporting
# a diagnostic, so retry once before declaring the link failed.
main_linked=1
main_attempt=1
while [ "$main_attempt" -le 2 ]; do
    # `-Itarget/cxxbridge` exposes the cxx-generated bridge header (created by
    # build_bridge_staticlib.sh above) under its canonical source-relative path.
    if "$CXX" -std=c++20 -Wall -Wextra -I. -Isrc -Itarget/cxxbridge \
        tests/native_plugin_compile_contract.cpp \
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
