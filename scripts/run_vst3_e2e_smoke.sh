#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
REQUIRE_VST3=${AURA_REQUIRE_VST3:-0}
if [ -z "${AURA_VST3_SDK:-}" ]; then
    for candidate in \
        "$ROOT_DIR/third_party/vst3sdk" \
        "/tmp/aura-vst3-sdk-new" \
        "$HOME/Developer/vst3sdk"
    do
        if [ -f "$candidate/CMakeLists.txt" ] && [ -d "$candidate/public.sdk" ]; then
            AURA_VST3_SDK="$candidate"
            break
        fi
    done
fi
if [ -z "${AURA_VST3_SDK:-}" ]; then
    if [ "$REQUIRE_VST3" = "1" ] || [ "${AURA_RELEASE_MODE:-0}" = "1" ]; then
        echo "AURA_VST3_SDK must point to the official Steinberg VST3 SDK" >&2
        exit 1
    fi
    echo "VST3 E2E: SKIPPED (AURA_VST3_SDK is not configured)"
    exit 0
fi
export AURA_VST3_SDK

if [ -z "${AURA_VST3_FIXTURE:-}" ]; then
    for candidate in \
        "$HOME/Library/Audio/Plug-Ins/VST3/Surge XT.vst3" \
        "$HOME/Library/Audio/Plug-Ins/VST3"/*.vst3
    do
        if [ -e "$candidate" ]; then
            AURA_VST3_FIXTURE="$candidate"
            break
        fi
    done
fi
if [ -z "${AURA_VST3_FIXTURE:-}" ]; then
    if [ "$REQUIRE_VST3" = "1" ] || [ "${AURA_RELEASE_MODE:-0}" = "1" ]; then
        echo "AURA_VST3_FIXTURE must point to an SDK-enabled VST3 fixture" >&2
        exit 1
    fi
    echo "VST3 E2E: SKIPPED (no SDK-enabled fixture found)"
    exit 0
fi

test -e "$AURA_VST3_FIXTURE"
export AURA_VST3_FIXTURE
AURA_PLUGIN_WORKER_OUTPUT="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3" \
    "$ROOT_DIR/scripts/build_plugin_worker_with_vst3.sh"

PLUGIN_TEST_BIN=${AURA_PLUGIN_TEST_BIN:-}
if [ -z "$PLUGIN_TEST_BIN" ]; then
    cargo test -p aura-core-bridge --test plugin_sandbox_workflow --no-run --quiet
    PLUGIN_TEST_BIN=$(find "$ROOT_DIR/target/debug/deps" -type f -perm -111 \
        -name 'plugin_sandbox_workflow-*' -print0 | xargs -0 ls -t 2>/dev/null | head -n 1)
fi
if [ -z "$PLUGIN_TEST_BIN" ] || [ ! -x "$PLUGIN_TEST_BIN" ]; then
    echo "unable to locate plugin sandbox E2E test binary" >&2
    exit 2
fi

run_plugin_test() {
    test_name=$1
    shift
    if ! "$PLUGIN_TEST_BIN" --list 2>/dev/null | grep -Eq "^${test_name}: test$"; then
        echo "plugin sandbox test is missing from selected binary: $test_name" >&2
        exit 2
    fi
    env "$@" "$PLUGIN_TEST_BIN" "$test_name" --exact --ignored --test-threads=1
}

run_plugin_test \
    official_vst3_fixture_instantiates_and_processes_in_the_isolated_worker \
    AURA_PLUGIN_HOST_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3" \
    AURA_VST3_FIXTURE="$AURA_VST3_FIXTURE"

run_plugin_test \
    isolated_plugin_worker_survives_audio_reconfiguration \
    AURA_PLUGIN_HOST_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3" \
    AURA_VST3_FIXTURE="$AURA_VST3_FIXTURE"

# Real instrument evidence: state restore must still produce finite note audio,
# not merely acknowledge a state blob or survive a worker restart.
run_plugin_test \
    real_instrument_state_restore_keeps_note_audio_finite \
    AURA_PLUGIN_HOST_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3" \
    AURA_INSTRUMENT_FIXTURE="$AURA_VST3_FIXTURE"

run_plugin_test \
    real_plugin_worker_fault_is_quarantined_without_nonfinite_audio \
    AURA_PLUGIN_TEST_FAULTS=1 \
    AURA_PLUGIN_WORKER_CRASH_AFTER_BLOCKS=2 \
    AURA_PLUGIN_HOST_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3" \
    AURA_VST3_FIXTURE="$AURA_VST3_FIXTURE"

run_plugin_test \
    real_plugin_overruns_quarantine_then_recover_audio_and_midi \
    AURA_PLUGIN_TEST_FAULTS=1 \
    AURA_PLUGIN_WORKER_DELAY_MS=50 \
    AURA_PLUGIN_HOST_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3" \
    AURA_VST3_FIXTURE="$AURA_VST3_FIXTURE"

echo "VST3 isolated-worker E2E smoke passed"
