#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "AU E2E smoke requires macOS" >&2
    exit 1
fi

if ! command -v auval >/dev/null 2>&1; then
    echo "auval is unavailable; refusing to report AU E2E success" >&2
    exit 1
fi

# Validate the same concrete component that the host contract instantiates.
# A compile-only AU path must never be reported as an AU E2E pass.
if ! auval -v aufx dcmp appl; then
    echo "auval rejected the Apple A Dynamics Processor component" >&2
    exit 1
fi

# The in-process AU contract above proves the Apple host API path.  When an
# actual component bundle is supplied, also exercise Aura's isolated worker.
# In strict release mode, silently omitting this second path would make an AU
# release claim too broad, so the fixture is required there.
if [ -z "${AURA_AU_FIXTURE:-}" ]; then
    for candidate in \
        "$HOME/Library/Audio/Plug-Ins/Components"/*.component \
        "/Library/Audio/Plug-Ins/Components"/*.component
    do
        if [ -e "$candidate" ]; then
            AURA_AU_FIXTURE="$candidate"
            break
        fi
    done
fi
if [ -z "${AURA_AU_FIXTURE:-}" ]; then
    if [ "${AURA_RELEASE_MODE:-0}" = "1" ]; then
        echo "AURA_AU_FIXTURE must point to a real .component bundle in release mode" >&2
        exit 1
    fi
    echo "AU isolated-worker E2E: SKIPPED (set AURA_AU_FIXTURE to a .component bundle)"
else
    case "$AURA_AU_FIXTURE" in
        *.component) : ;;
        *) echo "AURA_AU_FIXTURE must point to a .component bundle" >&2; exit 2 ;;
    esac
    test -e "$AURA_AU_FIXTURE"
fi

AU_HOST_BIN="${AURA_PLUGIN_HOST_BIN:-$ROOT_DIR/build-tools/aura-plugin-host-worker}"

OUT=${TMPDIR:-/tmp}/aura-au-host-e2e
c++ -std=c++20 -Wall -Wextra -I. -Isrc \
    tests/au_host_e2e_contract.cpp \
    -framework AudioToolbox -framework AudioUnit -framework CoreAudio -framework CoreFoundation \
    -o "$OUT"

ROUNDS=${AURA_AU_STRESS_ROUNDS:-1}
case "$ROUNDS" in ''|*[!0-9]*) echo "AURA_AU_STRESS_ROUNDS must be numeric" >&2; exit 2 ;; esac
[ "$ROUNDS" -gt 0 ] || { echo "AURA_AU_STRESS_ROUNDS must be positive" >&2; exit 2; }
i=1
while [ "$i" -le "$ROUNDS" ]; do
    "$OUT"
    i=$((i + 1))
done
echo "AU host E2E smoke passed (Apple A Dynamics Processor, mono/stereo, rounds=$ROUNDS)"

if [ -n "${AURA_AU_FIXTURE:-}" ]; then
    AU_PLUGIN_TEST_BIN=${AURA_PLUGIN_TEST_BIN:-}
    if [ -z "$AU_PLUGIN_TEST_BIN" ]; then
        cargo test -p aura-core-bridge --test plugin_sandbox_workflow --no-run --quiet
        AU_PLUGIN_TEST_BIN=$(find "$ROOT_DIR/target/debug/deps" -type f -perm -111 \
            -name 'plugin_sandbox_workflow-*' -print0 | xargs -0 ls -t 2>/dev/null | head -n 1)
    fi
    if [ -z "$AU_PLUGIN_TEST_BIN" ] || [ ! -x "$AU_PLUGIN_TEST_BIN" ]; then
        echo "unable to locate plugin sandbox AU E2E test binary" >&2
        exit 2
    fi
    run_plugin_test() {
        test_name=$1
        shift
        if ! "$AU_PLUGIN_TEST_BIN" --list 2>/dev/null | grep -Eq "^${test_name}: test$"; then
            echo "AU plugin sandbox test is missing from selected binary: $test_name" >&2
            exit 2
        fi
        env "$@" "$AU_PLUGIN_TEST_BIN" "$test_name" --exact --ignored --test-threads=1
    }
    run_plugin_test \
        isolated_plugin_worker_survives_audio_reconfiguration \
        AURA_PLUGIN_HOST_BIN="$AU_HOST_BIN" \
        AURA_AU_FIXTURE="$AURA_AU_FIXTURE"
    run_plugin_test \
        real_instrument_state_restore_keeps_note_audio_finite \
        AURA_PLUGIN_HOST_BIN="$AU_HOST_BIN" \
        AURA_INSTRUMENT_FIXTURE="$AURA_AU_FIXTURE"
    run_plugin_test \
        real_plugin_worker_fault_is_quarantined_without_nonfinite_audio \
        AURA_PLUGIN_TEST_FAULTS=1 \
        AURA_PLUGIN_WORKER_CRASH_AFTER_BLOCKS=2 \
        AURA_PLUGIN_HOST_BIN="$AU_HOST_BIN" \
        AURA_AU_FIXTURE="$AURA_AU_FIXTURE"
    run_plugin_test \
        real_plugin_overruns_quarantine_then_recover_audio_and_midi \
        AURA_PLUGIN_TEST_FAULTS=1 \
        AURA_PLUGIN_WORKER_DELAY_MS=100 \
        AURA_PLUGIN_HOST_BIN="$AU_HOST_BIN" \
        AURA_AU_FIXTURE="$AURA_AU_FIXTURE"
    echo "AU isolated-worker E2E passed: $AURA_AU_FIXTURE"
fi
