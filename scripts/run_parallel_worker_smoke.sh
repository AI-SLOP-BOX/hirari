#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
FIXTURE=${AURA_CLAP_FIXTURE:-$ROOT_DIR/build-tools/minimal-gain.clap}
WORKER=${AURA_PLUGIN_HOST_BIN:-$ROOT_DIR/build-tools/aura-plugin-host-worker}

test -f "$FIXTURE"
test -x "$WORKER"

cargo test -p aura-core-bridge --test plugin_sandbox_workflow --no-run --quiet
TEST_BIN=$(find "$ROOT_DIR/target/debug/deps" -type f -perm -111 \
    -name 'plugin_sandbox_workflow-*' -print0 | xargs -0 ls -t 2>/dev/null | head -n 1)
test -x "$TEST_BIN"
for TEST_NAME in \
    minimal_clap_fixture_instantiates_in_the_isolated_worker \
    minimal_clap_fixture_processes_continuous_audio_blocks_without_stale_output
do
    if ! "$TEST_BIN" --list 2>/dev/null | grep -Eq "^${TEST_NAME}: test$"; then
        echo "parallel worker smoke test is missing from selected binary: $TEST_NAME" >&2
        exit 2
    fi
done

first_log=$(mktemp "${TMPDIR:-/tmp}/aura-parallel-worker-1.XXXXXX")
second_log=$(mktemp "${TMPDIR:-/tmp}/aura-parallel-worker-2.XXXXXX")
cleanup() {
    rm -f "$first_log" "$second_log"
}
trap cleanup EXIT INT TERM

AURA_CLAP_FIXTURE="$FIXTURE" AURA_PLUGIN_HOST_BIN="$WORKER" AURA_PLUGIN_PATHS="$ROOT_DIR/build-tools" \
    "$TEST_BIN" minimal_clap_fixture_instantiates_in_the_isolated_worker \
    --exact --ignored --test-threads=1 >"$first_log" 2>&1 &
first_pid=$!
AURA_CLAP_FIXTURE="$FIXTURE" AURA_PLUGIN_HOST_BIN="$WORKER" AURA_PLUGIN_PATHS="$ROOT_DIR/build-tools" \
    "$TEST_BIN" minimal_clap_fixture_processes_continuous_audio_blocks_without_stale_output \
    --exact --ignored --test-threads=1 >"$second_log" 2>&1 &
second_pid=$!

first_status=0
second_status=0
wait "$first_pid" || first_status=$?
wait "$second_pid" || second_status=$?

if [ "$first_status" -ne 0 ] || [ "$second_status" -ne 0 ]; then
    cat "$first_log" >&2
    cat "$second_log" >&2
    echo "parallel worker smoke failed: first=$first_status second=$second_status" >&2
    exit 1
fi

echo "parallel worker smoke passed: two independent worker processes"
